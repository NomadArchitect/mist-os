// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::time::utc;
use fuchsia_runtime::UtcInstant;
use starnix_uapi::errors::Errno;
use starnix_uapi::time::{itimerspec_from_deadline_interval, time_from_timespec};
use starnix_uapi::{itimerspec, timespec};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timeline {
    RealTime,
    Monotonic,
    BootInstant,
}

impl Timeline {
    /// Returns the current time on this timeline.
    pub fn now(&self) -> TargetTime {
        match self {
            Self::RealTime => TargetTime::RealTime(utc::utc_now()),
            Self::Monotonic => TargetTime::Monotonic(zx::MonotonicInstant::get()),
            // TODO(https://fxbug.dev/328306129) handle boot and monotonic time separately
            Self::BootInstant => TargetTime::BootInstant(zx::MonotonicInstant::get()),
        }
    }

    pub fn target_from_timespec(&self, spec: timespec) -> Result<TargetTime, Errno> {
        Ok(match self {
            Timeline::Monotonic => TargetTime::Monotonic(time_from_timespec(spec)?),
            Timeline::RealTime => TargetTime::RealTime(time_from_timespec(spec)?),
            Timeline::BootInstant => TargetTime::BootInstant(time_from_timespec(spec)?),
        })
    }

    pub fn zero_time(&self) -> TargetTime {
        match self {
            Timeline::Monotonic => TargetTime::Monotonic(zx::Instant::from_nanos(0)),
            Timeline::RealTime => TargetTime::RealTime(zx::Instant::from_nanos(0)),
            Timeline::BootInstant => TargetTime::BootInstant(zx::Instant::from_nanos(0)),
        }
    }
}

#[derive(Debug)]
pub enum TimerWakeup {
    /// A regular timer that does not wake the system if it is suspended.
    Regular,
    /// An alarm timer that will wake the system if it is suspended.
    Alarm,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetTime {
    Monotonic(zx::MonotonicInstant),
    RealTime(UtcInstant),
    // TODO(https://fxbug.dev/328306129) handle boot time with its own type
    BootInstant(zx::MonotonicInstant),
}

impl TargetTime {
    pub fn is_zero(&self) -> bool {
        0 == match self {
            TargetTime::Monotonic(t) => t.into_nanos(),
            TargetTime::RealTime(t) => t.into_nanos(),
            TargetTime::BootInstant(t) => t.into_nanos(),
        }
    }

    pub fn itimerspec(&self, interval: zx::Duration) -> itimerspec {
        match self {
            TargetTime::Monotonic(t) | TargetTime::BootInstant(t) => {
                itimerspec_from_deadline_interval(*t, interval)
            }
            TargetTime::RealTime(t) => itimerspec_from_deadline_interval(*t, interval),
        }
    }

    // TODO(https://fxbug.dev/328306129) handle boot and monotonic time properly
    pub fn estimate_monotonic(&self) -> zx::MonotonicInstant {
        match self {
            TargetTime::BootInstant(t) | TargetTime::Monotonic(t) => *t,
            TargetTime::RealTime(t) => utc::estimate_monotonic_deadline_from_utc(*t),
        }
    }

    /// Find the difference between this time and `rhs`. Returns `None` if the timelines don't
    /// match.
    pub fn delta(&self, rhs: &Self) -> Option<zx::Duration> {
        match (*self, *rhs) {
            (TargetTime::Monotonic(lhs), TargetTime::Monotonic(rhs)) => Some(lhs - rhs),
            (TargetTime::BootInstant(lhs), TargetTime::BootInstant(rhs)) => Some(lhs - rhs),
            (TargetTime::RealTime(lhs), TargetTime::RealTime(rhs)) => Some(lhs - rhs),
            _ => None,
        }
    }
}

impl std::ops::Add<zx::Duration> for TargetTime {
    type Output = Self;
    fn add(self, rhs: zx::Duration) -> Self {
        match self {
            Self::RealTime(t) => Self::RealTime(t + rhs),
            Self::Monotonic(t) => Self::Monotonic(t + rhs),
            Self::BootInstant(t) => Self::BootInstant(t + rhs),
        }
    }
}

impl std::ops::Sub<zx::Duration> for TargetTime {
    type Output = zx::Duration;
    fn sub(self, rhs: zx::Duration) -> Self::Output {
        match self {
            TargetTime::Monotonic(t) => zx::Duration::from_nanos((t - rhs).into_nanos()),
            TargetTime::RealTime(t) => zx::Duration::from_nanos((t - rhs).into_nanos()),
            TargetTime::BootInstant(t) => zx::Duration::from_nanos((t - rhs).into_nanos()),
        }
    }
}

impl std::cmp::PartialOrd for TargetTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Self::Monotonic(lhs), Self::Monotonic(rhs)) => Some(lhs.cmp(rhs)),
            (Self::RealTime(lhs), Self::RealTime(rhs)) => Some(lhs.cmp(rhs)),
            (Self::BootInstant(lhs), Self::BootInstant(rhs)) => Some(lhs.cmp(rhs)),
            _ => None,
        }
    }
}
