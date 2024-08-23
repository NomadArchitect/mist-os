// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security::selinux_hooks::{
    check_permission, check_self_permission, fs_node_effective_sid, FsNodeHandle, PermissionCheck,
    ProcessPermission, TaskAttrs,
};
use crate::security::{Arc, ProcAttr, ResolvedElfState, SecurityServer};
use crate::task::{CurrentTask, Task};
use selinux::{FilePermission, NullessByteStr, ObjectClass};
use starnix_logging::log_debug;
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::{Signal, SIGCHLD, SIGKILL, SIGSTOP};
use starnix_uapi::{errno, error};

/// Returns `TaskAttrs` for a new `Task`, based on the `parent` state, and the specified clone flags.
pub fn task_alloc(parent: &Task, _clone_flags: u64) -> TaskAttrs {
    parent.read().security_state.attrs.clone()
}

/// Checks if creating a task is allowed.
pub fn check_task_create_access(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
) -> Result<(), Errno> {
    let task_sid = current_task.read().security_state.attrs.current_sid;
    check_self_permission(permission_check, task_sid, ProcessPermission::Fork)
}

/// Checks the SELinux permissions required for exec. Returns the SELinux state of a resolved
/// elf if all required permissions are allowed.
pub fn check_exec_access(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    executable_node: &FsNodeHandle,
) -> Result<ResolvedElfState, Errno> {
    let (current_sid, exec_sid) = {
        let state = &current_task.read().security_state.attrs;
        (state.current_sid, state.exec_sid)
    };

    let executable_sid = fs_node_effective_sid(security_server, current_task, executable_node);

    let new_sid = if let Some(exec_sid) = exec_sid {
        // Use the proc exec SID if set.
        exec_sid
    } else {
        security_server
            .compute_new_sid(current_sid, executable_sid, ObjectClass::Process)
            .map_err(|_| errno!(EACCES))?
    };
    if current_sid == new_sid {
        // To `exec()` a binary in the caller's domain, the caller must be granted
        // "execute_no_trans" permission to the binary.
        if !security_server.has_permission(
            current_sid,
            executable_sid,
            FilePermission::ExecuteNoTrans,
        ) {
            // TODO(http://b/330904217): once filesystems are labeled, deny access.
            log_debug!("execute_no_trans permission is denied, ignoring.");
        }
    } else {
        // Domain transition, check that transition is allowed.
        if !security_server.has_permission(current_sid, new_sid, ProcessPermission::Transition) {
            return error!(EACCES);
        }
        // Check that the executable file has an entry point into the new domain.
        if !security_server.has_permission(new_sid, executable_sid, FilePermission::Entrypoint) {
            // TODO(http://b/330904217): once filesystems are labeled, deny access.
            log_debug!("entrypoint permission is denied, ignoring.");
        }
        // Check that ptrace permission is allowed if the process is traced.
        if let Some(ptracer) = current_task.ptracer_task().upgrade() {
            let tracer_sid = ptracer.read().security_state.attrs.current_sid;
            if !security_server.has_permission(tracer_sid, new_sid, ProcessPermission::Ptrace) {
                return error!(EACCES);
            }
        }
    }
    Ok(ResolvedElfState { sid: Some(new_sid) })
}

/// Checks if source with `source_sid` may exercise the "getsched" permission on target with
/// `target_sid` according to SELinux server status `status` and permission checker
/// `permission`.
pub fn check_getsched_access(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    check_permission(permission_check, source_sid, target_sid, ProcessPermission::GetSched)
}

/// Checks if the task with `source_sid` is allowed to set scheduling parameters for the task with
/// `target_sid`.
pub fn check_setsched_access(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    check_permission(permission_check, source_sid, target_sid, ProcessPermission::SetSched)
}

/// Checks if the task with `source_sid` is allowed to get the process group ID of the task with
/// `target_sid`.
pub fn check_getpgid_access(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    check_permission(permission_check, source_sid, target_sid, ProcessPermission::GetPgid)
}

/// Checks if the task with `source_sid` is allowed to set the process group ID of the task with
/// `target_sid`.
pub fn check_setpgid_access(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    check_permission(permission_check, source_sid, target_sid, ProcessPermission::SetPgid)
}

/// Checks if the task with `source_sid` has permission to read the session Id from a task with `target_sid`.
/// Corresponds to the `task_getsid` LSM hook.
pub fn check_task_getsid(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    check_permission(permission_check, source_sid, target_sid, ProcessPermission::GetSession)
}

/// Checks if the task with `source_sid` is allowed to send `signal` to the task with `target_sid`.
pub fn check_signal_access(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
    signal: Signal,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    match signal {
        // The `sigkill` permission is required for sending SIGKILL.
        SIGKILL => {
            check_permission(permission_check, source_sid, target_sid, ProcessPermission::SigKill)
        }
        // The `sigstop` permission is required for sending SIGSTOP.
        SIGSTOP => {
            check_permission(permission_check, source_sid, target_sid, ProcessPermission::SigStop)
        }
        // The `sigchld` permission is required for sending SIGCHLD.
        SIGCHLD => {
            check_permission(permission_check, source_sid, target_sid, ProcessPermission::SigChld)
        }
        // The `signal` permission is required for sending any signal other than SIGKILL, SIGSTOP
        // or SIGCHLD.
        _ => check_permission(permission_check, source_sid, target_sid, ProcessPermission::Signal),
    }
}

/// Returns the serialized Security Context associated with the specified task.
pub fn task_get_context(
    security_server: &SecurityServer,
    _current_task: &CurrentTask,
    target: &Task,
) -> Result<Vec<u8>, Errno> {
    let sid = target.read().security_state.attrs.current_sid;
    Ok(security_server.sid_to_security_context(sid).unwrap_or_default())
}

/// Checks if the task with `source_sid` has the permission to get and/or set limits on the task with `target_sid`.
pub fn task_prlimit(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    target: &Task,
    check_get_rlimit: bool,
    check_set_rlimit: bool,
) -> Result<(), Errno> {
    let source_sid = current_task.read().security_state.attrs.current_sid;
    let target_sid = target.read().security_state.attrs.current_sid;
    if check_get_rlimit {
        check_permission(permission_check, source_sid, target_sid, ProcessPermission::GetRlimit)?;
    }
    if check_set_rlimit {
        check_permission(permission_check, source_sid, target_sid, ProcessPermission::SetRlimit)?;
    }
    Ok(())
}

/// Checks if the task with `source_sid` is allowed to trace the task with `target_sid`.
pub fn ptrace_access_check(
    permission_check: &impl PermissionCheck,
    current_task: &CurrentTask,
    tracee: &Task,
) -> Result<(), Errno> {
    let tracer_sid = current_task.read().security_state.attrs.current_sid;
    let tracee_sid = tracee.read().security_state.attrs.current_sid;
    check_permission(permission_check, tracer_sid, tracee_sid, ProcessPermission::Ptrace)
}

/// Returns the Security Context associated with the `name`ed entry for the specified `target` task.
/// `source` describes the calling task, `target` the state of the task for which to return the attribute.
pub fn get_procattr(
    security_server: &SecurityServer,
    _current_task: &CurrentTask,
    task: &Task,
    attr: ProcAttr,
) -> Result<Vec<u8>, Errno> {
    let task_attrs = &task.read().security_state.attrs;

    let sid = match attr {
        ProcAttr::Current => Some(task_attrs.current_sid),
        ProcAttr::Exec => task_attrs.exec_sid,
        ProcAttr::FsCreate => task_attrs.fscreate_sid,
        ProcAttr::KeyCreate => task_attrs.keycreate_sid,
        ProcAttr::Previous => Some(task_attrs.previous_sid),
        ProcAttr::SockCreate => task_attrs.sockcreate_sid,
    };

    // Convert it to a Security Context string.
    Ok(sid.and_then(|sid| security_server.sid_to_security_context(sid)).unwrap_or_default())
}

/// Sets the Security Context associated with the `attr` entry in the task security state.
pub fn set_procattr(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    attr: ProcAttr,
    context: &[u8],
) -> Result<(), Errno> {
    // Attempt to convert the Security Context string to a SID.
    let context = NullessByteStr::from(context);
    let sid = match context.as_bytes() {
        b"\x0a" | b"" => None,
        _ => Some(security_server.security_context_to_sid(context).map_err(|_| errno!(EINVAL))?),
    };

    let permission_check = security_server.as_permission_check();
    let current_sid = current_task.read().security_state.attrs.current_sid;
    match attr {
        ProcAttr::Current => {
            check_self_permission(&permission_check, current_sid, ProcessPermission::SetCurrent)?;

            // Permission to dynamically transition to the new Context is also required.
            let new_sid = sid.ok_or_else(|| errno!(EINVAL))?;
            check_permission(
                &permission_check,
                current_sid,
                new_sid,
                ProcessPermission::DynTransition,
            )?;

            if current_task.thread_group.read().tasks_count() > 1 {
                // In multi-threaded programs dynamic transitions may only be used to down-scope
                // the capabilities available to the task. This is verified by requiring an explicit
                // "typebounds" relationship between the current and target domains, indicating that
                // the constraint on permissions of the bounded type has been verified by the policy
                // build tooling and/or will be enforced at run-time on permission checks.
                if !security_server.is_bounded_by(new_sid, current_sid) {
                    return error!(EACCES);
                }
            }

            current_task.write().security_state.attrs.current_sid = new_sid
        }
        ProcAttr::Previous => {
            return error!(EINVAL);
        }
        ProcAttr::Exec => {
            check_self_permission(&permission_check, current_sid, ProcessPermission::SetExec)?;
            current_task.write().security_state.attrs.exec_sid = sid
        }
        ProcAttr::FsCreate => {
            check_self_permission(&permission_check, current_sid, ProcessPermission::SetFsCreate)?;
            current_task.write().security_state.attrs.fscreate_sid = sid
        }
        ProcAttr::KeyCreate => {
            check_self_permission(&permission_check, current_sid, ProcessPermission::SetKeyCreate)?;
            current_task.write().security_state.attrs.keycreate_sid = sid
        }
        ProcAttr::SockCreate => {
            check_self_permission(
                &permission_check,
                current_sid,
                ProcessPermission::SetSockCreate,
            )?;
            current_task.write().security_state.attrs.sockcreate_sid = sid
        }
    };

    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::security::selinux_hooks::testing::create_test_executable;
    use crate::security::selinux_hooks::{testing, InitialSid, TaskAttrs};
    use crate::security::update_state_on_exec;
    use crate::testing::{
        create_kernel_and_task_with_selinux, create_kernel_task_and_unlocked_with_selinux,
        create_task,
    };
    use selinux_core::security_server::Mode;
    use selinux_core::SecurityId;
    use starnix_uapi::signals::SIGTERM;
    use starnix_uapi::{error, CLONE_SIGHAND, CLONE_THREAD, CLONE_VM};

    #[fuchsia::test]
    async fn task_alloc_from_parent() {
        let security_server = testing::security_server_with_policy();
        let (_kernel, current_task) = create_kernel_and_task_with_selinux(security_server.clone());

        // Create a fake parent state, with values for some fields, to check for.
        let parent_security_state = TaskAttrs {
            current_sid: SecurityId::initial(InitialSid::Unlabeled),
            previous_sid: SecurityId::initial(InitialSid::Kernel),
            exec_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
            fscreate_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
            keycreate_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
            sockcreate_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
        };

        current_task.write().security_state.attrs = parent_security_state.clone();

        let security_state = task_alloc(&current_task, 0);
        assert_eq!(security_state, parent_security_state);
    }

    #[fuchsia::test]
    fn task_alloc_for() {
        let for_kernel = TaskAttrs::for_kernel();
        assert_eq!(for_kernel.current_sid, SecurityId::initial(InitialSid::Kernel));
        assert_eq!(for_kernel.previous_sid, for_kernel.current_sid);
        assert_eq!(for_kernel.exec_sid, None);
        assert_eq!(for_kernel.fscreate_sid, None);
        assert_eq!(for_kernel.keycreate_sid, None);
        assert_eq!(for_kernel.sockcreate_sid, None);
    }

    #[fuchsia::test]
    async fn task_create_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (_kernel, current_task) = create_kernel_and_task_with_selinux(security_server.clone());
        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:fork_yes_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_task_create_access(&security_server.as_permission_check(), &current_task),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn task_create_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let (_kernel, current_task) = create_kernel_and_task_with_selinux(security_server.clone());
        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:fork_no_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_task_create_access(&security_server.as_permission_check(), &current_task),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    async fn exec_transition_allowed_for_allowed_transition_type() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let current_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
            .expect("invalid security context");
        let exec_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
            .expect("invalid security context");

        let executable_security_context = b"u:object_r:executable_file_trans_t:s0";
        assert!(security_server
            .security_context_to_sid(executable_security_context.into())
            .is_ok());
        let executable =
            create_test_executable(&mut locked, &current_task, executable_security_context);
        let executable_fs_node = &executable.entry.node;

        current_task.write().security_state.attrs = TaskAttrs {
            current_sid: current_sid,
            exec_sid: Some(exec_sid),
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: current_sid,
            sockcreate_sid: None,
        };

        assert_eq!(
            check_exec_access(&security_server, &current_task, executable_fs_node),
            Ok(ResolvedElfState { sid: Some(exec_sid) })
        );
    }

    #[fuchsia::test]
    async fn exec_transition_denied_for_transition_denied_type() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let current_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
            .expect("invalid security context");
        let exec_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_denied_target_t:s0".into())
            .expect("invalid security context");

        let executable_security_context = b"u:object_r:executable_file_trans_t:s0";
        assert!(security_server
            .security_context_to_sid(executable_security_context.into())
            .is_ok());
        let executable =
            create_test_executable(&mut locked, &current_task, executable_security_context);
        let executable_fs_node = &executable.entry.node;

        current_task.write().security_state.attrs = TaskAttrs {
            current_sid: current_sid,
            exec_sid: Some(exec_sid),
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: current_sid,
            sockcreate_sid: None,
        };

        assert_eq!(
            check_exec_access(&security_server, &current_task, executable_fs_node),
            error!(EACCES)
        );
    }

    // TODO(http://b/330904217): reenable test once filesystems are labeled and access is denied.
    #[ignore]
    #[fuchsia::test]
    async fn exec_transition_denied_for_executable_with_no_entrypoint_perm() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let current_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
            .expect("invalid security context");
        let exec_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
            .expect("invalid security context");

        let executable_security_context = b"u:object_r:executable_file_trans_no_entrypoint_t:s0";
        assert!(security_server
            .security_context_to_sid(executable_security_context.into())
            .is_ok());
        let executable =
            create_test_executable(&mut locked, &current_task, executable_security_context);
        let executable_fs_node = &executable.entry.node;

        current_task.write().security_state.attrs = TaskAttrs {
            current_sid: current_sid,
            exec_sid: Some(exec_sid),
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: current_sid,
            sockcreate_sid: None,
        };

        assert_eq!(
            check_exec_access(&security_server, &current_task, executable_fs_node),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    async fn exec_no_trans_allowed_for_executable() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());

        let current_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_no_trans_source_t:s0".into())
            .expect("invalid security context");

        let executable_security_context = b"u:object_r:executable_file_no_trans_t:s0";
        assert!(security_server
            .security_context_to_sid(executable_security_context.into())
            .is_ok());
        let executable =
            create_test_executable(&mut locked, &current_task, executable_security_context);
        let executable_fs_node = &executable.entry.node;

        current_task.write().security_state.attrs = TaskAttrs {
            current_sid: current_sid,
            exec_sid: None,
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: current_sid,
            sockcreate_sid: None,
        };

        assert_eq!(
            check_exec_access(&security_server, &current_task, executable_fs_node),
            Ok(ResolvedElfState { sid: Some(current_sid) })
        );
    }

    // TODO(http://b/330904217): reenable test once filesystems are labeled and access is denied.
    #[ignore]
    #[fuchsia::test]
    async fn exec_no_trans_denied_for_executable() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let current_sid = security_server
            .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
            .expect("invalid security context");

        let executable_security_context = b"u:object_r:executable_file_no_trans_t:s0";
        assert!(security_server
            .security_context_to_sid(executable_security_context.into())
            .is_ok());
        let executable =
            create_test_executable(&mut locked, &current_task, executable_security_context);
        let executable_fs_node = &executable.entry.node;

        current_task.write().security_state.attrs = TaskAttrs {
            current_sid: current_sid,
            exec_sid: None,
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: current_sid,
            sockcreate_sid: None,
        };

        // There is no `execute_no_trans` allow statement from `current_sid` to `executable_sid`,
        // expect access denied.
        assert_eq!(
            check_exec_access(&security_server, &current_task, executable_fs_node),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    async fn state_is_updated_on_exec() {
        let security_server = testing::security_server_with_policy();
        let (_kernel, current_task) = create_kernel_and_task_with_selinux(security_server.clone());

        let initial_state = {
            let state = &mut current_task.write().security_state.attrs;

            // Set previous SID to a different value from current, to allow verification
            // of the pre-exec "current" being moved into "previous".
            state.previous_sid = SecurityId::initial(InitialSid::Unlabeled);

            // Set the other optional SIDs to a value, to verify that it is cleared on exec update.
            state.sockcreate_sid = Some(SecurityId::initial(InitialSid::Unlabeled));
            state.fscreate_sid = Some(SecurityId::initial(InitialSid::Unlabeled));
            state.keycreate_sid = Some(SecurityId::initial(InitialSid::Unlabeled));

            state.clone()
        };

        // Ensure that the ELF binary SID differs from the task's current SID before exec.
        let elf_sid = security_server
            .security_context_to_sid(b"u:object_r:test_valid_t:s0".into())
            .expect("invalid security context");
        assert_ne!(elf_sid, initial_state.current_sid);

        update_state_on_exec(&current_task, &ResolvedElfState { sid: Some(elf_sid) });
        assert_eq!(
            current_task.read().security_state.attrs,
            TaskAttrs {
                current_sid: elf_sid,
                exec_sid: None,
                fscreate_sid: None,
                keycreate_sid: None,
                previous_sid: initial_state.current_sid,
                sockcreate_sid: None,
            }
        );
    }

    #[fuchsia::test]
    async fn setsched_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_yes_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_setsched_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn setsched_access_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_no_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_setsched_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task
            ),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    async fn getsched_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_yes_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getsched_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn getsched_access_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_no_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getsched_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task
            ),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    async fn getpgid_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_yes_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getpgid_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn getpgid_access_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_no_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getpgid_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task
            ),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    async fn sigkill_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_sigkill_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task,
                SIGKILL,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn sigchld_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_sigchld_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task,
                SIGCHLD,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn sigstop_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_sigstop_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task,
                SIGSTOP,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn signal_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_signal_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        // The `signal` permission allows signals other than SIGKILL, SIGCHLD, SIGSTOP.
        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                &current_task,
                &target_task,
                SIGTERM,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn signal_access_denied_for_denied_signals() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let target_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_signal_t:s0".into())
            .expect("invalid security context");
        target_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        // The `signal` permission does not allow SIGKILL, SIGCHLD or SIGSTOP.
        for signal in [SIGCHLD, SIGKILL, SIGSTOP] {
            assert_eq!(
                check_signal_access(
                    &security_server.as_permission_check(),
                    &current_task,
                    &target_task,
                    signal,
                ),
                error!(EACCES)
            );
        }
    }

    #[fuchsia::test]
    async fn ptrace_access_allowed_for_allowed_type_and_state_is_updated() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let tracee_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_ptrace_tracer_yes_t:s0".into())
            .expect("invalid security context");
        {
            let attrs = &mut tracee_task.write().security_state.attrs;
            attrs.current_sid = security_server
                .security_context_to_sid(b"u:object_r:test_ptrace_traced_t:s0".into())
                .expect("invalid security context");
            attrs.previous_sid = attrs.current_sid;
        }

        assert_eq!(
            ptrace_access_check(
                &security_server.as_permission_check(),
                &current_task,
                &tracee_task,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn ptrace_access_denied_for_denied_type_and_state_is_not_updated() {
        let security_server = testing::security_server_with_policy();
        let (kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let tracee_task = create_task(&mut unlocked, &kernel, "target_task");

        current_task.write().security_state.attrs.current_sid = security_server
            .security_context_to_sid(b"u:object_r:test_ptrace_tracer_no_t:s0".into())
            .expect("invalid security context");
        {
            let attrs = &mut tracee_task.write().security_state.attrs;
            attrs.current_sid = security_server
                .security_context_to_sid(b"u:object_r:test_ptrace_traced_t:s0".into())
                .expect("invalid security context");
            attrs.previous_sid = attrs.current_sid;
        }

        assert_eq!(
            ptrace_access_check(
                &security_server.as_permission_check(),
                &current_task,
                &tracee_task,
            ),
            error!(EACCES)
        );
        // TODO: Verify that the tracer has not been set on `tracee_task`.
    }

    #[fuchsia::test]
    async fn setcurrent_bounds() {
        const BINARY_POLICY: &[u8] = include_bytes!("../../../lib/selinux/testdata/composite_policies/compiled/bounded_transition_policy.pp");
        const BOUNDED_CONTEXT: &[u8] = b"test_u:test_r:bounded_t:s0";
        const UNBOUNDED_CONTEXT: &[u8] = b"test_u:test_r:unbounded_t:s0";

        let security_server = SecurityServer::new(Mode::Enable);
        security_server.set_enforcing(true);
        security_server.load_policy(BINARY_POLICY.to_vec()).expect("policy load failed");
        let unbounded_sid = security_server
            .security_context_to_sid(UNBOUNDED_CONTEXT.into())
            .expect("Make unbounded SID");

        let (_kernel, current_task, mut unlocked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        current_task.write().security_state.attrs.current_sid = unbounded_sid;

        // Thread-group has a single task, so dynamic transitions are permitted, with "setcurrent"
        // and "dyntransition".
        assert_eq!(
            set_procattr(&security_server, &current_task, ProcAttr::Current, BOUNDED_CONTEXT),
            Ok(()),
            "Unbounded_t->bounded_t single-threaded"
        );
        assert_eq!(
            set_procattr(&security_server, &current_task, ProcAttr::Current, UNBOUNDED_CONTEXT),
            Ok(()),
            "Bounded_t->unbounded_t single-threaded"
        );

        // Create a second task in the same thread group.
        let _child_task = current_task.clone_task_for_test(
            &mut unlocked,
            (CLONE_THREAD | CLONE_VM | CLONE_SIGHAND) as u64,
            None,
        );

        // Thread-group has a multiple tasks, so dynamic transitions to are only allowed to bounded
        // domains.
        assert_eq!(
            set_procattr(&security_server, &current_task, ProcAttr::Current, BOUNDED_CONTEXT),
            Ok(()),
            "Unbounded_t->bounded_t multi-threaded"
        );
        assert_eq!(
            set_procattr(&security_server, &current_task, ProcAttr::Current, UNBOUNDED_CONTEXT),
            error!(EACCES),
            "Bounded_t->unbounded_t multi-threaded"
        );
    }
}
