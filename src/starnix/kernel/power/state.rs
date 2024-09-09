// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::{BytesFile, BytesFileOps, FsNodeOps};
use fidl_fuchsia_power_broker::PowerLevel;
#[cfg(feature = "wake_locks")]
use fuchsia_zircon as zx;
use itertools::Itertools;
#[cfg(not(feature = "wake_locks"))]
use starnix_logging::log_warn;
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::borrow::Cow;

#[cfg(feature = "wake_locks")]
use fidl_fuchsia_starnix_runner as frunner;
#[cfg(feature = "wake_locks")]
use fuchsia_component::client::connect_to_protocol_sync;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum SuspendState {
    /// Suspend-to-disk
    ///
    /// This state offers the greatest energy savings.
    Disk,
    /// Suspend-to-Ram
    ///
    /// This state, if supported, offers significant power savings as everything in the system is
    /// put into a low-power state, except for memory.
    Ram,
    /// Standby
    ///
    /// This state, if supported, offers moderate, but real, energy savings, while providing a
    /// relatively straightforward transition back to the working state.
    ///
    Standby,
    /// Suspend-To-Idle
    ///
    /// This state is a generic, pure software, light-weight, system sleep state.
    Idle,
}

impl SuspendState {
    pub fn to_str(&self) -> &'static str {
        match self {
            SuspendState::Disk => "disk",
            SuspendState::Ram => "mem",
            SuspendState::Idle => "freeze",
            SuspendState::Standby => "standby",
        }
    }
}

impl TryFrom<&str> for SuspendState {
    type Error = Errno;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(match value {
            "disk" => SuspendState::Disk,
            "mem" => SuspendState::Ram,
            "standby" => SuspendState::Standby,
            "freeze" => SuspendState::Idle,
            _ => return error!(EINVAL),
        })
    }
}

impl From<SuspendState> for PowerLevel {
    fn from(value: SuspendState) -> Self {
        match value {
            SuspendState::Disk => 0,
            SuspendState::Ram => 1,
            SuspendState::Standby => 2,
            SuspendState::Idle => 3,
        }
    }
}

pub struct PowerStateFile;

impl PowerStateFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for PowerStateFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let state_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let clean_state_str = state_str.split('\n').next().unwrap_or("");
        let state: SuspendState = clean_state_str.try_into()?;

        let power_manager = &current_task.kernel().suspend_resume_manager;
        let supported_states = power_manager.suspend_states();
        if !supported_states.contains(&state) {
            return error!(EINVAL);
        }
        fuchsia_trace::duration!(c"power", c"starnix-sysfs:suspend");
        #[cfg(not(feature = "wake_locks"))]
        {
            power_manager.suspend(state).inspect_err(|e| log_warn!("Suspend failed: {e}"))?;
        }

        #[cfg(feature = "wake_locks")]
        {
            let manager = connect_to_protocol_sync::<frunner::ManagerMarker>()
                .expect("Failed to connect to manager");
            manager
                .suspend_container(
                    frunner::ManagerSuspendContainerRequest {
                        container_job: Some(
                            fuchsia_runtime::job_default()
                                .duplicate(zx::Rights::SAME_RIGHTS)
                                .expect("Failed to dup handle"),
                        ),
                        wake_event: current_task.kernel().hrtimer_manager.duplicate_timer_event(),
                        wake_locks: Some(
                            current_task.kernel().suspend_resume_manager.duplicate_lock_event(),
                        ),
                        ..Default::default()
                    },
                    zx::Time::INFINITE,
                )
                .map_err(|_| errno!(EINVAL))?
                .map_err(|_| errno!(EINVAL))?;
        }
        Ok(())
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let states = current_task.kernel().suspend_resume_manager.suspend_states();
        let content = states.iter().map(SuspendState::to_str).join(" ") + "\n";
        Ok(content.as_bytes().to_owned().into())
    }
}
