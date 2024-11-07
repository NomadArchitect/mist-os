// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::future::{Future, FutureExt, LocalFutureObj};
use futures::ready;
use futures::task::{Context, Poll};
use std::convert::Infallible;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct ExitReason(pub Result<(), anyhow::Error>);

pub struct State<E>(LocalFutureObj<'static, Result<State<E>, E>>);

pub struct StateMachine<E> {
    cur_state: State<E>,
}

impl<E> Unpin for StateMachine<E> {}

impl<E> Future for StateMachine<E> {
    type Output = Result<Infallible, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.cur_state.0.poll_unpin(cx)) {
                Ok(next) => self.cur_state = next,
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }
}

pub trait IntoStateExt<E>: Future<Output = Result<State<E>, E>> {
    fn into_state(self) -> State<E>
    where
        Self: Sized + 'static,
    {
        State(LocalFutureObj::new(Box::new(self)))
    }

    fn into_state_machine(self) -> StateMachine<E>
    where
        Self: Sized + 'static,
    {
        StateMachine { cur_state: self.into_state() }
    }
}

impl<F, E> IntoStateExt<E> for F where F: Future<Output = Result<State<E>, E>> {}

// Some helpers to allow state machines to publish state and other futures to check in on the most
// recent state updates.
#[derive(Clone)]
pub struct StateMachineStatusPublisher<S>(Arc<RwLock<S>>);

impl<S: Clone + Debug> StateMachineStatusPublisher<S> {
    pub fn publish_status(&self, status: S) {
        match self.0.write() {
            Ok(mut writer) => *writer = status,
            Err(e) => tracing::warn!("Failed to write current status {:?}: {}", status, e),
        }
    }
}

#[derive(Clone)]
pub struct StateMachineStatusReader<S>(Arc<RwLock<S>>);

impl<S: Clone + Debug> StateMachineStatusReader<S> {
    pub fn read_status(&self) -> Result<S, anyhow::Error> {
        match self.0.read() {
            Ok(reader) => Ok(reader.clone()),
            Err(e) => Err(anyhow::format_err!("Failed to read current status: {}", e)),
        }
    }
}

pub fn status_publisher_and_reader<S: Clone + Default>(
) -> (StateMachineStatusPublisher<S>, StateMachineStatusReader<S>) {
    let status = Arc::new(RwLock::new(S::default()));
    (StateMachineStatusPublisher(status.clone()), StateMachineStatusReader(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use futures::channel::mpsc;
    use futures::stream::StreamExt;
    use std::mem;

    #[fuchsia::test]
    fn state_machine() {
        let mut exec = fasync::TestExecutor::new();
        let (sender, receiver) = mpsc::unbounded();
        let mut state_machine = sum_state(0, receiver).into_state_machine();

        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut state_machine));

        sender.unbounded_send(2).unwrap();
        sender.unbounded_send(3).unwrap();
        mem::drop(sender);

        assert_eq!(Poll::Ready(Err(5)), exec.run_until_stalled(&mut state_machine));
    }

    async fn sum_state(
        current: u32,
        stream: mpsc::UnboundedReceiver<u32>,
    ) -> Result<State<u32>, u32> {
        let (number, stream) = stream.into_future().await;
        match number {
            Some(number) => Ok(make_sum_state(current + number, stream)),
            None => Err(current),
        }
    }

    // A workaround for the "recursive impl Trait" problem in the compiler
    fn make_sum_state(current: u32, stream: mpsc::UnboundedReceiver<u32>) -> State<u32> {
        sum_state(current, stream).into_state()
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    enum FakeState {
        #[default]
        Beginning,
        Middle,
        End,
    }

    #[fuchsia::test]
    fn state_publish_and_read() {
        let _exec = fasync::TestExecutor::new();
        let (publisher, reader) = status_publisher_and_reader::<FakeState>();
        assert_eq!(reader.read_status().expect("failed to read status"), FakeState::Beginning);

        publisher.publish_status(FakeState::Middle);
        assert_eq!(reader.read_status().expect("failed to read status"), FakeState::Middle);

        publisher.publish_status(FakeState::End);
        assert_eq!(reader.read_status().expect("failed to read status"), FakeState::End);
    }
}
