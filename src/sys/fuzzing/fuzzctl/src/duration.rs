// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This module exists to abstract away the differences between the dev host and target versions
/// of `fuchsia_async::MonotonicDuration`.
///
/// In particular, on development hosts `fuchsia_async::{MonotonicDuration, MonotonicInstant}` are actually
/// `std::time::{Duration, Instant}`.
pub use self::platform::{deadline_after, MonotonicDuration};

#[cfg(not(target_os = "fuchsia"))]
mod platform {
    use std::time::{Duration as OsDuration, Instant as OsTime};

    #[derive(Debug)]
    pub struct MonotonicDuration {
        base: OsDuration,
    }

    impl MonotonicDuration {
        pub const fn from_nanos(nanos: i64) -> Self {
            Self { base: OsDuration::from_nanos(nanos as u64) }
        }

        pub const fn from_micros(micros: i64) -> Self {
            Self { base: OsDuration::from_micros(micros as u64) }
        }

        pub const fn from_millis(millis: i64) -> Self {
            Self { base: OsDuration::from_millis(millis as u64) }
        }

        pub const fn from_seconds(seconds: i64) -> Self {
            Self { base: OsDuration::from_secs(seconds as u64) }
        }

        pub const fn into_nanos(self) -> i64 {
            self.base.as_nanos() as i64
        }

        pub const fn into_seconds(self) -> i64 {
            self.base.as_secs() as i64
        }
    }

    /// Provides a deadline after `timeout` nanoseconds that a `fuchsia_async::Timer` can wait until.
    pub fn deadline_after(timeout: Option<i64>) -> Option<fuchsia_async::MonotonicInstant> {
        timeout.and_then(|nanos| OsTime::now().checked_add(OsDuration::from_nanos(nanos as u64)))
    }
}

#[cfg(target_os = "fuchsia")]
mod platform {
    use fuchsia_async::DurationExt;
    pub use fuchsia_async::MonotonicDuration;

    /// Provides a deadline after `timeout` nanoseconds that a `fuchsia_async::Timer` can wait until.
    pub fn deadline_after(timeout: Option<i64>) -> Option<fuchsia_async::MonotonicInstant> {
        timeout.map(|nanos| MonotonicDuration::from_nanos(nanos).after_now())
    }
}
