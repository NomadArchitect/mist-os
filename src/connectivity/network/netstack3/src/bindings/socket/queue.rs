// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Queue for datagram-like sockets.

use std::collections::VecDeque;

use log::{error, trace};
use thiserror::Error;

// These values were picked to match Linux behavior.

/// Limits the total size of messages that can be queued for an application
/// socket to be read before we start dropping packets.
pub(crate) const MAX_OUTSTANDING_APPLICATION_MESSAGES_SIZE: usize = 4 * 1024 * 1024;
/// The default value for the amount of data that can be queued for an
/// application socket to be read before packets are dropped.
pub(crate) const DEFAULT_OUTSTANDING_APPLICATION_MESSAGES_SIZE: usize = 208 * 1024;
/// The minimum value for the amount of data that can be queued for an
/// application socket to be read before packets are dropped.
pub(crate) const MIN_OUTSTANDING_APPLICATION_MESSAGES_SIZE: usize = 256;

#[derive(Copy, Clone, Debug, Error, Eq, PartialEq)]
#[error("application buffers are full")]
pub(crate) struct NoSpace;

/// A trait abstracting types that are notified of the queue being readable.
///
/// Upon creation, the listener must assume to be *not* readable.
pub(crate) trait QueueReadableListener {
    /// Notifies the listener of a readable change.
    fn on_readable_changed(&mut self, readable: bool);
}

#[derive(Debug)]
pub(crate) struct MessageQueue<M, L> {
    listener: L,
    queue: AvailableMessageQueue<M>,
}

impl<M, L: QueueReadableListener> MessageQueue<M, L> {
    pub(crate) fn new(listener: L) -> Self {
        Self {
            listener,
            queue: AvailableMessageQueue::new(DEFAULT_OUTSTANDING_APPLICATION_MESSAGES_SIZE),
        }
    }

    pub(crate) fn peek(&self) -> Option<&M> {
        let Self { queue, listener: _ } = self;
        queue.peek()
    }

    pub(crate) fn pop(&mut self) -> Option<M>
    where
        M: BodyLen,
    {
        let Self { queue, listener } = self;
        let message = queue.pop();
        // NB: Only notify the listener when the queue was not empty before to
        // avoid hitting the listener twice with the same signal.
        if queue.is_empty() && message.is_some() {
            listener.on_readable_changed(false);
        }
        message
    }

    pub(crate) fn receive(&mut self, message: M)
    where
        M: BodyLen,
    {
        let Self { queue, listener } = self;
        let body_len = message.body_len();
        let queue_was_empty = queue.is_empty();
        match queue.push(message) {
            Err(NoSpace) => {
                trace!("dropping {}-byte packet because the receive queue is full", body_len)
            }
            Ok(()) => {
                // NB: If the queue is non-empty, it would be redundant to
                // signal the event. Avoid the unnecessary syscall.
                // This is a safe optimization because signals are only set
                // on the event while holding an `&mut MessageQueue`.
                if queue_was_empty {
                    listener.on_readable_changed(true);
                }
            }
        }
    }

    pub(crate) fn listener_mut(&mut self) -> &mut L {
        &mut self.listener
    }

    pub(crate) fn max_available_messages_size(&self) -> usize {
        let Self { listener: _, queue } = self;
        queue.max_available_messages_size
    }

    pub(crate) fn set_max_available_messages_size(&mut self, new_size: usize) {
        let Self { listener: _, queue } = self;
        queue.max_available_messages_size = usize::max(
            usize::min(new_size, MAX_OUTSTANDING_APPLICATION_MESSAGES_SIZE),
            MIN_OUTSTANDING_APPLICATION_MESSAGES_SIZE,
        );
    }

    #[cfg(test)]
    pub(crate) fn available_messages(&self) -> impl ExactSizeIterator<Item = &M> {
        let Self {
            listener: _,
            queue:
                AvailableMessageQueue {
                    available_messages,
                    available_messages_size: _,
                    max_available_messages_size: _,
                },
        } = self;
        available_messages.iter()
    }
}

#[derive(Debug)]
struct AvailableMessageQueue<M> {
    available_messages: VecDeque<M>,
    /// The total size of the contents of `available_messages`.
    available_messages_size: usize,
    /// The maximum allowed value for `available_messages_size`.
    max_available_messages_size: usize,
}

pub(crate) trait BodyLen {
    fn body_len(&self) -> usize;
}

impl<M> AvailableMessageQueue<M> {
    pub(crate) fn new(max_available_messages_size: usize) -> Self {
        Self {
            available_messages: Default::default(),
            available_messages_size: 0,
            max_available_messages_size,
        }
    }

    pub(crate) fn push(&mut self, message: M) -> Result<(), NoSpace>
    where
        M: BodyLen,
    {
        let Self { available_messages, available_messages_size, max_available_messages_size } =
            self;

        // Respect the configured limit except if this would be the only message
        // in the buffer. This is compatible with Linux behavior.
        let len = message.body_len();
        if *available_messages_size + len > *max_available_messages_size
            && !available_messages.is_empty()
        {
            return Err(NoSpace);
        }

        available_messages.push_back(message);
        *available_messages_size += len;
        Ok(())
    }

    pub(crate) fn pop(&mut self) -> Option<M>
    where
        M: BodyLen,
    {
        let Self { available_messages, available_messages_size, max_available_messages_size: _ } =
            self;

        available_messages.pop_front().map(|msg| {
            *available_messages_size -= msg.body_len();
            msg
        })
    }

    pub(crate) fn peek(&self) -> Option<&M> {
        let Self { available_messages, available_messages_size: _, max_available_messages_size: _ } =
            self;
        available_messages.front()
    }

    pub(crate) fn is_empty(&self) -> bool {
        let Self { available_messages, available_messages_size: _, max_available_messages_size: _ } =
            self;
        available_messages.is_empty()
    }
}
