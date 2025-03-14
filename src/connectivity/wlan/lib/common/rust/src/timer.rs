// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::{FutureExt, Stream, StreamExt};

use crate::sink::UnboundedSink;

pub type ScheduledEvent<E> = (zx::MonotonicInstant, Event<E>);
pub type EventSender<E> = UnboundedSink<ScheduledEvent<E>>;
pub type EventStream<E> = mpsc::UnboundedReceiver<ScheduledEvent<E>>;
pub type EventId = u64;

// The returned timer will send scheduled timeouts to the returned EventStream.
// Note that this will not actually have any timed behavior unless events are pulled off
// the EventStream and handled asynchronously.
pub fn create_timer<E>() -> (Timer<E>, EventStream<E>) {
    let (timer_sink, time_stream) = mpsc::unbounded();
    (Timer::new(UnboundedSink::new(timer_sink)), time_stream)
}

pub fn make_async_timed_event_stream<E>(
    time_stream: impl Stream<Item = ScheduledEvent<E>>,
) -> impl Stream<Item = Event<E>> {
    time_stream
        .map(|(deadline, timed_event)| {
            fasync::Timer::new(fasync::MonotonicInstant::from_zx(deadline)).map(|_| timed_event)
        })
        .buffer_unordered(usize::max_value())
}

#[derive(Debug)]
pub struct Event<E> {
    pub id: EventId,
    pub event: E,
}

impl<E: Clone> Clone for Event<E> {
    fn clone(&self) -> Self {
        Event { id: self.id, event: self.event.clone() }
    }
}

#[derive(Debug)]
pub struct Timer<E> {
    sender: EventSender<E>,
    next_id: EventId,
}

impl<E> Timer<E> {
    pub fn new(sender: EventSender<E>) -> Self {
        Timer { sender, next_id: 0 }
    }

    /// Returns the current time according to the global executor.
    ///
    /// # Panics
    ///
    /// This function will panic if it's called when no executor is set up.
    pub fn now(&self) -> zx::MonotonicInstant {
        // We use fasync to support time manipulation in tests.
        fasync::MonotonicInstant::now().into_zx()
    }

    pub fn schedule_at(&mut self, deadline: zx::MonotonicInstant, event: E) -> EventId {
        let id = self.next_id;
        self.sender.send((deadline, Event { id, event }));
        self.next_id += 1;
        id
    }

    pub fn schedule_after(&mut self, duration: zx::MonotonicDuration, event: E) -> EventId {
        self.schedule_at(fasync::MonotonicInstant::after(duration).into_zx(), event)
    }

    pub fn schedule<EV>(&mut self, event: EV) -> EventId
    where
        EV: TimeoutDuration + Into<E>,
    {
        self.schedule_after(event.timeout_duration(), event.into())
    }
}

pub trait TimeoutDuration {
    fn timeout_duration(&self) -> zx::MonotonicDuration;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_variant;
    use fuchsia_async as fasync;

    use futures::channel::mpsc::UnboundedSender;
    use std::pin::pin;
    use std::task::Poll;

    type TestEvent = u32;
    impl TimeoutDuration for TestEvent {
        fn timeout_duration(&self) -> zx::MonotonicDuration {
            zx::MonotonicDuration::from_seconds(10)
        }
    }

    #[test]
    fn test_timer_schedule_at() {
        let _exec = fasync::TestExecutor::new();
        let (mut timer, mut time_stream) = create_timer::<TestEvent>();
        let timeout1 = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5));
        let timeout2 = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(10));
        assert_eq!(timer.schedule_at(timeout1, 7), 0);
        assert_eq!(timer.schedule_at(timeout2, 9), 1);

        let (t1, event1) = time_stream.try_next().unwrap().expect("expect time entry");
        assert_eq!(t1, timeout1);
        assert_eq!(event1.id, 0);
        assert_eq!(event1.event, 7);

        let (t2, event2) = time_stream.try_next().unwrap().expect("expect time entry");
        assert_eq!(t2, timeout2);
        assert_eq!(event2.id, 1);
        assert_eq!(event2.event, 9);

        assert_variant!(time_stream.try_next(), Err(e) => {
            assert_eq!(e.to_string(), "receiver channel is empty")
        });
    }

    #[test]
    fn test_timer_schedule_after() {
        let _exec = fasync::TestExecutor::new();
        let (mut timer, mut time_stream) = create_timer::<TestEvent>();
        let timeout1 = zx::MonotonicDuration::from_seconds(1000);
        let timeout2 = zx::MonotonicDuration::from_seconds(5);
        assert_eq!(timer.schedule_after(timeout1, 7), 0);
        assert_eq!(timer.schedule_after(timeout2, 9), 1);

        let (t1, event1) = time_stream.try_next().unwrap().expect("expect time entry");
        assert_eq!(event1.id, 0);
        assert_eq!(event1.event, 7);

        let (t2, event2) = time_stream.try_next().unwrap().expect("expect time entry");
        assert_eq!(event2.id, 1);
        assert_eq!(event2.event, 9);

        // Confirm that the ordering of timeouts is expected. We can't check the actual
        // values since they're dependent on the system clock.
        assert!(t1.into_nanos() > t2.into_nanos());

        assert_variant!(time_stream.try_next(), Err(e) => {
            assert_eq!(e.to_string(), "receiver channel is empty")
        });
    }

    #[test]
    fn test_timer_schedule() {
        let _exec = fasync::TestExecutor::new();
        let (mut timer, mut time_stream) = create_timer::<TestEvent>();
        let start = zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(0));

        assert_eq!(timer.schedule(5u32), 0);

        let (t, event) = time_stream.try_next().unwrap().expect("expect time entry");
        assert_eq!(event.id, 0);
        assert_eq!(event.event, 5);
        assert!(start + zx::MonotonicDuration::from_seconds(10) <= t);
    }

    #[test]
    fn test_timer_stream() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let fut = async {
            let (timer, time_stream) = mpsc::unbounded::<ScheduledEvent<TestEvent>>();
            let mut timeout_stream = make_async_timed_event_stream(time_stream);
            let now = zx::MonotonicInstant::get();
            schedule(&timer, now + zx::MonotonicDuration::from_millis(40), 0);
            schedule(&timer, now + zx::MonotonicDuration::from_millis(10), 1);
            schedule(&timer, now + zx::MonotonicDuration::from_millis(20), 2);
            schedule(&timer, now + zx::MonotonicDuration::from_millis(30), 3);

            let mut events = vec![];
            for _ in 0u32..4 {
                let event = timeout_stream.next().await.expect("timer terminated prematurely");
                events.push(event.event);
            }
            events
        };
        let mut fut = pin!(fut);
        for _ in 0u32..4 {
            assert_eq!(Poll::Pending, exec.run_until_stalled(&mut fut));
            assert!(exec.wake_next_timer().is_some());
        }
        assert_variant!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(events) => assert_eq!(events, vec![1, 2, 3, 0]),
        );
    }

    fn schedule(
        timer: &UnboundedSender<ScheduledEvent<TestEvent>>,
        deadline: zx::MonotonicInstant,
        event: TestEvent,
    ) {
        let entry = (deadline, Event { id: 0, event });
        timer.unbounded_send(entry).expect("expect send successful");
    }
}
