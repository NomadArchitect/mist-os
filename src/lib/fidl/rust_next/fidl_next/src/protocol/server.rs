// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL protocol servers.

use core::num::NonZeroU32;

use crate::protocol::{decode_header, encode_header, DispatcherError, Transport};
use crate::{Encode, EncodeError, EncoderExt as _};

/// A responder for a transactional request.
#[must_use]
pub struct Responder {
    txid: NonZeroU32,
}

/// A sender for a server endpoint.
pub struct Server<T: Transport> {
    sender: T::Sender,
}

impl<T: Transport> Server<T> {
    /// Creates a new server and dispatcher from a transport.
    pub fn new(transport: T) -> (Self, ServerDispatcher<T>) {
        let (sender, receiver) = transport.split();
        (Self { sender }, ServerDispatcher { receiver })
    }

    /// Closes the channel from the server end.
    pub fn close(&self) {
        T::close(&self.sender);
    }

    /// Send an event.
    pub fn send_event<M>(
        &self,
        ordinal: u64,
        event: &mut M,
    ) -> Result<T::SendFuture<'_>, EncodeError>
    where
        M: for<'a> Encode<T::Encoder<'a>>,
    {
        let mut buffer = T::acquire(&self.sender);
        encode_header::<T>(&mut buffer, 0, ordinal)?;
        T::encoder(&mut buffer).encode_next(event)?;
        Ok(T::send(&self.sender, buffer))
    }

    /// Send a response to a transactional request.
    pub fn send_response<M>(
        &self,
        responder: Responder,
        ordinal: u64,
        response: &mut M,
    ) -> Result<T::SendFuture<'_>, EncodeError>
    where
        M: for<'a> Encode<T::Encoder<'a>>,
    {
        let mut buffer = T::acquire(&self.sender);
        encode_header::<T>(&mut buffer, responder.txid.get(), ordinal)?;
        T::encoder(&mut buffer).encode_next(response)?;
        Ok(T::send(&self.sender, buffer))
    }
}

impl<T: Transport> Clone for Server<T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone() }
    }
}

/// A type which handles incoming events for a server.
pub trait ServerHandler<T: Transport> {
    /// Handles a received server event.
    ///
    /// The dispatcher cannot handle more messages until `on_event` completes. If `on_event` may
    /// block, perform asynchronous work, or take a long time to process a message, it should
    /// offload work to an async task.
    fn on_event(&mut self, ordinal: u64, buffer: T::RecvBuffer);

    /// Handles a received server transaction.
    ///
    /// The dispatcher cannot handle more messages until `on_event` completes. If `on_event` may
    /// block, perform asynchronous work, or take a long time to process a message, it should
    /// offload work to an async task.
    fn on_transaction(&mut self, ordinal: u64, buffer: T::RecvBuffer, responder: Responder);
}

/// A dispatcher for a server endpoint.
pub struct ServerDispatcher<T: Transport> {
    receiver: T::Receiver,
}

impl<T: Transport> ServerDispatcher<T> {
    /// Runs the dispatcher with the provided handler.
    pub async fn run<H>(&mut self, mut handler: H) -> Result<(), DispatcherError<T::Error>>
    where
        H: ServerHandler<T>,
    {
        while let Some(mut buffer) =
            T::recv(&mut self.receiver).await.map_err(DispatcherError::TransportError)?
        {
            let (txid, ordinal) =
                decode_header::<T>(&mut buffer).map_err(DispatcherError::InvalidMessageHeader)?;
            if let Some(txid) = NonZeroU32::new(txid) {
                handler.on_transaction(ordinal, buffer, Responder { txid });
            } else {
                handler.on_event(ordinal, buffer);
            }
        }

        Ok(())
    }
}
