// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(super) mod fs;
pub(super) mod testing;

use super::{FsNodeSecurityXattr, FsNodeState, ProcAttr, ResolvedElfState};
use crate::task::{CurrentTask, Task};
use crate::vfs::{FsNode, FsNodeHandle, FsStr, FsString, NamespaceNode, ValueOrSize, XattrOp};
use linux_uapi::XATTR_NAME_SELINUX;
use selinux::permission_check::PermissionCheck;
use selinux::security_server::SecurityServer;
use selinux::{InitialSid, SecurityId};
use selinux_common::{
    ClassPermission, FilePermission, NullessByteStr, ObjectClass, Permission, ProcessPermission,
};
use starnix_logging::{log_debug, track_stub};
use starnix_uapi::errors::Errno;
use starnix_uapi::mount_flags::MountFlags;
use starnix_uapi::signals::{Signal, SIGCHLD, SIGKILL, SIGSTOP};
use starnix_uapi::unmount_flags::UnmountFlags;
use starnix_uapi::{errno, error};
use std::collections::HashMap;
use std::sync::Arc;

/// Maximum supported size for the extended attribute value used to store SELinux security
/// contexts in a filesystem node extended attributes.
const SECURITY_SELINUX_XATTR_VALUE_MAX_SIZE: usize = 4096;

/// Checks if creating a task is allowed.
pub(super) fn check_task_create_access(
    permission_check: &impl PermissionCheck,
    task_sid: SecurityId,
) -> Result<(), Errno> {
    check_self_permissions(permission_check, task_sid, &[ProcessPermission::Fork])
}

/// Checks the SELinux permissions required for exec. Returns the SELinux state of a resolved
/// elf if all required permissions are allowed.
pub(super) fn check_exec_access(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    executable_node: &FsNodeHandle,
) -> Result<ResolvedElfState, Errno> {
    let (current_sid, exec_sid) = {
        let state = &current_task.read().security_state.attrs;
        (state.current_sid, state.exec_sid)
    };

    let executable_sid =
        get_effective_fs_node_security_id(security_server, current_task, executable_node);

    let new_sid = if let Some(exec_sid) = exec_sid {
        // Use the proc exec SID if set.
        exec_sid
    } else {
        security_server
            .compute_new_sid(current_sid, executable_sid, ObjectClass::Process)
            .map_err(|_| errno!(EACCES))?
        // TODO(http://b/319232900): validate that the new context is valid, and return EACCESS if
        // it's not.
    };
    if current_sid == new_sid {
        // To `exec()` a binary in the caller's domain, the caller must be granted
        // "execute_no_trans" permission to the binary.
        if !security_server.has_permissions(
            current_sid,
            executable_sid,
            &[FilePermission::ExecuteNoTrans],
        ) {
            // TODO(http://b/330904217): once filesystems are labeled, deny access.
            log_debug!("execute_no_trans permission is denied, ignoring.");
        }
    } else {
        // Domain transition, check that transition is allowed.
        if !security_server.has_permissions(current_sid, new_sid, &[ProcessPermission::Transition])
        {
            return error!(EACCES);
        }
        // Check that the executable file has an entry point into the new domain.
        if !security_server.has_permissions(new_sid, executable_sid, &[FilePermission::Entrypoint])
        {
            // TODO(http://b/330904217): once filesystems are labeled, deny access.
            log_debug!("entrypoint permission is denied, ignoring.");
        }
        // Check that ptrace permission is allowed if the process is traced.
        if let Some(ptracer) = current_task.ptracer_task().upgrade() {
            let tracer_sid = ptracer.read().security_state.attrs.current_sid;
            if !security_server.has_permissions(tracer_sid, new_sid, &[ProcessPermission::Ptrace]) {
                return error!(EACCES);
            }
        }
    }
    Ok(ResolvedElfState { sid: Some(new_sid) })
}

/// Updates the SELinux thread group state on exec, using the security ID associated with the
/// resolved elf.
pub(super) fn update_state_on_exec(
    current_task: &CurrentTask,
    elf_security_state: &ResolvedElfState,
) {
    let task_attrs = &mut current_task.write().security_state.attrs;
    let previous_sid = task_attrs.current_sid;

    *task_attrs = TaskAttrs {
        current_sid: elf_security_state
            .sid
            .expect("SELinux enabled but missing resolved elf state"),
        previous_sid,
        exec_sid: None,
        fscreate_sid: None,
        keycreate_sid: None,
        sockcreate_sid: None,
    };
}

/// Checks if source with `source_sid` may exercise the "getsched" permission on target with
/// `target_sid` according to SELinux server status `status` and permission checker
/// `permission`.
pub(super) fn check_getsched_access(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
) -> Result<(), Errno> {
    check_permissions(permission_check, source_sid, target_sid, &[ProcessPermission::GetSched])
}

/// Checks if the task with `source_sid` is allowed to set scheduling parameters for the task with
/// `target_sid`.
pub(super) fn check_setsched_access(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
) -> Result<(), Errno> {
    check_permissions(permission_check, source_sid, target_sid, &[ProcessPermission::SetSched])
}

/// Checks if the task with `source_sid` is allowed to get the process group ID of the task with
/// `target_sid`.
pub(super) fn check_getpgid_access(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
) -> Result<(), Errno> {
    check_permissions(permission_check, source_sid, target_sid, &[ProcessPermission::GetPgid])
}

/// Checks if the task with `source_sid` is allowed to set the process group ID of the task with
/// `target_sid`.
pub(super) fn check_setpgid_access(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
) -> Result<(), Errno> {
    check_permissions(permission_check, source_sid, target_sid, &[ProcessPermission::SetPgid])
}

/// Checks if the task with `source_sid` has permission to read the session Id from a task with `target_sid`.
/// Corresponds to the `task_getsid` LSM hook.
pub(super) fn check_task_getsid(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
) -> Result<(), Errno> {
    check_permissions(permission_check, source_sid, target_sid, &[ProcessPermission::GetSession])
}

/// Checks if the task with `source_sid` is allowed to send `signal` to the task with `target_sid`.
pub(super) fn check_signal_access(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
    signal: Signal,
) -> Result<(), Errno> {
    match signal {
        // The `sigkill` permission is required for sending SIGKILL.
        SIGKILL => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::SigKill],
        ),
        // The `sigstop` permission is required for sending SIGSTOP.
        SIGSTOP => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::SigStop],
        ),
        // The `sigchld` permission is required for sending SIGCHLD.
        SIGCHLD => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::SigChld],
        ),
        // The `signal` permission is required for sending any signal other than SIGKILL, SIGSTOP
        // or SIGCHLD.
        _ => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::Signal],
        ),
    }
}

/// Checks if the task with `source_sid` has the permission to get and/or set limits on the task with `target_sid`.
pub(super) fn task_prlimit(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
    check_get_rlimit: bool,
    check_set_rlimit: bool,
) -> Result<(), Errno> {
    match (check_get_rlimit, check_set_rlimit) {
        (true, true) => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::GetRlimit, ProcessPermission::SetRlimit],
        ),
        (true, false) => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::GetRlimit],
        ),
        (false, true) => check_permissions(
            permission_check,
            source_sid,
            target_sid,
            &[ProcessPermission::SetRlimit],
        ),
        (false, false) => Ok(()),
    }
}

/// Checks if the task with `_source_sid` has the permission to mount at `_path` the object specified by
/// `_dev_name` of type `_fs_type`, with the mounting flags `_flags` and filesystem data `_data`.
pub(super) fn sb_mount(
    _permission_check: &impl PermissionCheck,
    _source_sid: SecurityId,
    _dev_name: &bstr::BStr,
    _path: &NamespaceNode,
    _fs_type: &bstr::BStr,
    _flags: MountFlags,
    _data: &bstr::BStr,
) -> Result<(), Errno> {
    track_stub!(TODO("https://fxbug.dev/352507622"), "sb_mount: validate permission");
    Ok(())
}

/// Checks if the task with `_source_sid` has the permission to unmount the filesystem mounted on
/// `_node` using the unmount flags `_flags`.
pub(super) fn sb_umount(
    _permission_check: &impl PermissionCheck,
    _source_sid: SecurityId,
    _node: &NamespaceNode,
    _flags: UnmountFlags,
) -> Result<(), Errno> {
    track_stub!(TODO("https://fxbug.dev/353936182"), "sb_umount: validate permission");
    Ok(())
}

/// Checks if the task with `source_sid` is allowed to trace the task with `target_sid`.
pub(super) fn ptrace_access_check(
    permission_check: &impl PermissionCheck,
    tracer_sid: SecurityId,
    tracee_security_state: &mut TaskAttrs,
) -> Result<(), Errno> {
    check_permissions(
        permission_check,
        tracer_sid,
        tracee_security_state.current_sid,
        &[ProcessPermission::Ptrace],
    )
}

/// Returns the Security Context corresponding to the SID with which `FsNode`
/// is labelled, otherwise delegates to the node's [`crate::vfs::FsNodeOps`].
pub(super) fn fs_node_getsecurity(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
    max_size: usize,
) -> Result<ValueOrSize<FsString>, Errno> {
    if name == FsStr::new(XATTR_NAME_SELINUX.to_bytes()) {
        if let Some(sid) = fs_node.info().security_state.sid {
            if let Some(context) = security_server.sid_to_security_context(sid) {
                return Ok(ValueOrSize::Value(context.into()));
            }
        }
    }
    fs_node.ops().get_xattr(fs_node, current_task, name, max_size)
}

/// Sets the `name`d security attribute on `fs_node` and updates internal
/// kernel state.
pub(super) fn fs_node_setsecurity(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    fs_node: &FsNode,
    name: &FsStr,
    value: &FsStr,
    op: XattrOp,
) -> Result<(), Errno> {
    fs_node.ops().set_xattr(fs_node, current_task, name, value, op)?;
    if name == FsStr::new(XATTR_NAME_SELINUX.to_bytes()) {
        // Update or remove the SID from `fs_node`, dependent whether the new value
        // represents a valid Security Context.
        match security_server.security_context_to_sid(value.into()) {
            Ok(sid) => set_cached_sid(fs_node, sid),
            Err(_) => clear_cached_sid(fs_node),
        }
    }
    Ok(())
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
    // TODO(b/322849067): Validate that the `source` has the required access.

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
            check_self_permissions(
                &permission_check,
                current_sid,
                &[ProcessPermission::SetCurrent],
            )?;

            // Permission to dynamically transition to the new Context is also required.
            let new_sid = sid.ok_or_else(|| errno!(EINVAL))?;
            check_permissions(
                &permission_check,
                current_sid,
                new_sid,
                &[ProcessPermission::DynTransition],
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
            check_self_permissions(&permission_check, current_sid, &[ProcessPermission::SetExec])?;
            current_task.write().security_state.attrs.exec_sid = sid
        }
        ProcAttr::FsCreate => {
            check_self_permissions(
                &permission_check,
                current_sid,
                &[ProcessPermission::SetFsCreate],
            )?;
            current_task.write().security_state.attrs.fscreate_sid = sid
        }
        ProcAttr::KeyCreate => {
            check_self_permissions(
                &permission_check,
                current_sid,
                &[ProcessPermission::SetKeyCreate],
            )?;
            current_task.write().security_state.attrs.keycreate_sid = sid
        }
        ProcAttr::SockCreate => {
            check_self_permissions(
                &permission_check,
                current_sid,
                &[ProcessPermission::SetSockCreate],
            )?;
            current_task.write().security_state.attrs.sockcreate_sid = sid
        }
    };

    Ok(())
}

/// Determines the effective Security Context to use in access control checks on the supplied `fs_node`.
///
/// This logic is a work-in-progress but will involve (at least) the following:
///
/// 1. If the filesystem has a "context=" mount option, then cache that SID in the node.
// TODO(b/334091674): Implement the "context=" override.
/// 2. If the filesystem has "fs_use_xattr" then:
///    a. If the file has a "security.selinux" valid with the current policy then obtain the SID
///       and cache it.
///    b. If the file has a "security.selinux" invalid with the current policy then return the
///       "unlabeled" SID without caching.
///    c. If the file lacks a "security.selinux" attribute then check the filesystem's
///       "defcontext=" mount option; if set then return that SID, without caching.
// TODO(b/334091674): Implement the "defcontext=" override.
/// 3. If the policy defines security context(s) for the filesystem type on which `fs_node` resides
///    then use those to determine a SID, and cache it.
// TODO(b/334091674): Implement use of policy-defined contexts (e.g. via `genfscon`).
/// 4. Return the policy's "file" initial context.
fn compute_fs_node_security_id(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    fs_node: &FsNode,
) -> SecurityId {
    // TODO(b/334091674): Take into account "context" override here.

    // Use `fs_node.ops().get_xattr()` instead of `fs_node.get_xattr()` to bypass permission
    // checks performed on starnix userspace calls to get an extended attribute.
    match fs_node.ops().get_xattr(
        fs_node,
        current_task,
        XATTR_NAME_SELINUX.to_bytes().into(),
        SECURITY_SELINUX_XATTR_VALUE_MAX_SIZE,
    ) {
        Ok(ValueOrSize::Value(security_context)) => {
            match security_server.security_context_to_sid((&security_context).into()) {
                Ok(sid) => {
                    // Update node SID value if a SID is found to be associated with new security context
                    // string.
                    set_cached_sid(fs_node, sid);

                    sid
                }
                // TODO(b/330875626): What is the correct behaviour when no sid can be
                // constructed for the security context string (presumably because the context
                // string is invalid for the current policy)?
                _ => SecurityId::initial(InitialSid::Unlabeled),
            }
        }
        _ => {
            // TODO(b/334091674): Complete the fallback implementation (e.g. using the file system's "defcontext",
            // if specified).
            SecurityId::initial(InitialSid::File)
        }
    }
}

/// Checks if `permissions` are allowed from the task with `source_sid` to the task with `target_sid`.
fn check_permissions<P: ClassPermission + Into<Permission> + Clone + 'static>(
    permission_check: &impl PermissionCheck,
    source_sid: SecurityId,
    target_sid: SecurityId,
    permissions: &[P],
) -> Result<(), Errno> {
    match permission_check.has_permissions(source_sid, target_sid, permissions) {
        true => Ok(()),
        false => error!(EACCES),
    }
}

/// Checks that `subject_sid` has the specified process `permissions` on `self`.
fn check_self_permissions(
    permission_check: &impl PermissionCheck,
    subject_sid: SecurityId,
    permissions: &[ProcessPermission],
) -> Result<(), Errno> {
    check_permissions(permission_check, subject_sid, subject_sid, permissions)
}

/// Return security state to associate with a filesystem based on the supplied mount options.
pub fn file_system_init_security(
    fs_type: &FsStr,
    options: &HashMap<FsString, FsString>,
) -> Result<FileSystemState, Errno> {
    let context = options.get(FsStr::new(b"context")).cloned();
    let mut def_context = options.get(FsStr::new(b"defcontext")).cloned();
    let fs_context = options.get(FsStr::new(b"fscontext")).cloned();
    let root_context = options.get(FsStr::new(b"rootcontext")).cloned();

    // TODO(http://b/320436714): Remove this once policy-defined default-contexts are implemented.
    if **fs_type == *b"tmpfs" && def_context.is_none() && context.is_none() {
        def_context = Some(b"u:object_r:tmpfs:s0".into());
    }

    // If a "context" is specified then it is used for all nodes in the filesystem, so none of the other
    // security context options would be meaningful to combine with it.
    if context.is_some()
        && (def_context.is_some() || fs_context.is_some() || root_context.is_some())
    {
        return error!(EINVAL);
    }

    Ok(FileSystemState { context, def_context, fs_context, root_context })
}

/// Returns the security attribute to label a newly created inode with, if any.
pub fn fs_node_security_xattr(
    _security_server: &SecurityServer,
    new_node: &FsNodeHandle,
    _parent: Option<&FsNodeHandle>,
) -> Result<Option<FsNodeSecurityXattr>, Errno> {
    // TODO(b/334091674): If there is no `parent` then this is the "root" node; apply `root_context`, if set.
    // TODO(b/334091674): Determine whether "context" (and "defcontext") should be returned here, or only set in
    // the node's cached SID.
    let fs = new_node.fs();
    Ok(fs
        .security_state
        .state
        .context
        .as_ref()
        .or(fs.security_state.state.def_context.as_ref())
        .map(|context| FsNodeSecurityXattr {
            name: XATTR_NAME_SELINUX.to_bytes().into(),
            value: context.clone(),
        }))
}

/// Returns `TaskAttrs` for a new `Task`, based on the `parent` state, and the specified clone flags.
pub(super) fn task_alloc(parent: &TaskAttrs, _clone_flags: u64) -> TaskAttrs {
    parent.clone()
}

/// The SELinux security structure for `ThreadGroup`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TaskAttrs {
    /// Current SID for the task.
    pub current_sid: SecurityId,

    /// SID for the task upon the next execve call.
    pub exec_sid: Option<SecurityId>,

    /// SID for files created by the task.
    pub fscreate_sid: Option<SecurityId>,

    /// SID for kernel-managed keys created by the task.
    pub keycreate_sid: Option<SecurityId>,

    /// SID prior to the last execve.
    pub previous_sid: SecurityId,

    /// SID for sockets created by the task.
    pub sockcreate_sid: Option<SecurityId>,
}

impl TaskAttrs {
    /// Returns initial state for kernel tasks.
    pub(super) fn for_kernel() -> Self {
        Self::for_initial_sid(InitialSid::Kernel)
    }

    /// Returns placeholder state for use when SELinux is not enabled.
    pub(super) fn for_selinux_disabled() -> Self {
        Self::for_initial_sid(InitialSid::Unlabeled)
    }

    fn for_initial_sid(initial_sid: InitialSid) -> Self {
        Self {
            current_sid: SecurityId::initial(initial_sid),
            previous_sid: SecurityId::initial(initial_sid),
            exec_sid: None,
            fscreate_sid: None,
            keycreate_sid: None,
            sockcreate_sid: None,
        }
    }
}

/// SELinux security context-related filesystem mount options. These options are documented in the
/// `context=context, fscontext=context, defcontext=context, and rootcontext=context` section of
/// the `mount(8)` manpage.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FileSystemState {
    /// Specifies the effective security context to use for all nodes in the filesystem, and the
    /// filesystem itself. If the filesystem already contains security attributes then these are
    /// ignored. May not be combined with any of the other options.
    context: Option<FsString>,
    /// Specifies an effective security context to use for un-labeled nodes in the filesystem,
    /// rather than falling-back to the policy-defined "file" context.
    def_context: Option<FsString>,
    /// The value of the `fscontext=[security-context]` mount option. This option is used to
    /// label the filesystem (superblock) itself.
    fs_context: Option<FsString>,
    /// The value of the `rootcontext=[security-context]` mount option. This option is used to
    /// (re)label the inode located at the filesystem mountpoint.
    root_context: Option<FsString>,
}

/// Returns the security id that should be used for SELinux access control checks against `fs_node`
/// at this time. If no security id is cached, it is recomputed via `compute_fs_node_security_id()`.
fn get_effective_fs_node_security_id(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    fs_node: &FsNode,
) -> SecurityId {
    // Note: the sid is read before the match statement because otherwise the lock in
    // `self.info()` would be held for the duration of the match statement, leading to a
    // deadlock with `compute_fs_node_security_id()`.
    let sid = fs_node.info().security_state.sid;
    match sid {
        Some(sid) => sid,
        None => compute_fs_node_security_id(security_server, current_task, fs_node),
    }
}

/// Sets the cached security id associated with `fs_node` to `sid`. Storing the security id will
/// cause the security id to *not* be recomputed by the SELinux LSM when determining the effective
/// security id of this [`FsNode`].
fn set_cached_sid(fs_node: &FsNode, sid: SecurityId) {
    fs_node.update_info(|info| info.security_state = FsNodeState { sid: Some(sid) });
}

/// Clears the cached security id on `fs_node`. Clearing the security id will cause the security id
/// to be be recomputed by the SELinux LSM when determining the effective security id of this
/// [`FsNode`].
fn clear_cached_sid(fs_node: &FsNode) {
    fs_node.update_info(|info| info.security_state = FsNodeState { sid: None });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        create_kernel_and_task_with_selinux, create_kernel_task_and_unlocked_with_selinux,
        AutoReleasableTask,
    };
    use crate::vfs::{NamespaceNode, XattrOp};
    use selinux::security_server::Mode;
    use starnix_sync::{Locked, Unlocked};
    use starnix_uapi::device_type::DeviceType;
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::signals::SIGTERM;
    use starnix_uapi::{CLONE_SIGHAND, CLONE_THREAD, CLONE_VM};

    const VALID_SECURITY_CONTEXT: &[u8] = b"u:object_r:test_valid_t:s0";

    fn create_test_file(
        locked: &mut Locked<'_, Unlocked>,
        current_task: &AutoReleasableTask,
    ) -> NamespaceNode {
        current_task
            .fs()
            .root()
            .create_node(locked, &current_task, "file".into(), FileMode::IFREG, DeviceType::NONE)
            .expect("create_node(file)")
    }

    fn create_test_executable(
        locked: &mut Locked<'_, Unlocked>,
        current_task: &AutoReleasableTask,
        security_context: &[u8],
    ) -> NamespaceNode {
        let namespace_node = current_task
            .fs()
            .root()
            .create_node(
                locked,
                &current_task,
                "executable".into(),
                FileMode::IFREG,
                DeviceType::NONE,
            )
            .expect("create_node(file)");
        let fs_node = &namespace_node.entry.node;
        fs_node
            .ops()
            .set_xattr(
                fs_node,
                current_task,
                XATTR_NAME_SELINUX.to_bytes().into(),
                security_context.into(),
                XattrOp::Set,
            )
            .expect("set security.selinux xattr");
        namespace_node
    }

    #[fuchsia::test]
    fn task_create_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let sid = security_server
            .security_context_to_sid(b"u:object_r:fork_yes_t:s0".into())
            .expect("invalid security context");

        assert_eq!(check_task_create_access(&security_server.as_permission_check(), sid), Ok(()));
    }

    #[fuchsia::test]
    fn task_create_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let sid = security_server
            .security_context_to_sid(b"u:object_r:fork_no_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_task_create_access(&security_server.as_permission_check(), sid),
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
    fn setsched_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_yes_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_setsched_access(&security_server.as_permission_check(), source_sid, target_sid),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn setsched_access_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_no_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_setsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_setsched_access(&security_server.as_permission_check(), source_sid, target_sid),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    fn getsched_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_yes_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getsched_access(&security_server.as_permission_check(), source_sid, target_sid),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn getsched_access_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_no_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getsched_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getsched_access(&security_server.as_permission_check(), source_sid, target_sid),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    fn getpgid_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_yes_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getpgid_access(&security_server.as_permission_check(), source_sid, target_sid),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn getpgid_access_denied_for_denied_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_no_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_getpgid_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_getpgid_access(&security_server.as_permission_check(), source_sid, target_sid),
            error!(EACCES)
        );
    }

    #[fuchsia::test]
    fn sigkill_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_sigkill_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                source_sid,
                target_sid,
                SIGKILL,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn sigchld_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_sigchld_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                source_sid,
                target_sid,
                SIGCHLD,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn sigstop_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_sigstop_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                source_sid,
                target_sid,
                SIGSTOP,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn signal_access_allowed_for_allowed_type() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_signal_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        // The `signal` permission allows signals other than SIGKILL, SIGCHLD, SIGSTOP.
        assert_eq!(
            check_signal_access(
                &security_server.as_permission_check(),
                source_sid,
                target_sid,
                SIGTERM,
            ),
            Ok(())
        );
    }

    #[fuchsia::test]
    fn signal_access_denied_for_denied_signals() {
        let security_server = testing::security_server_with_policy();
        let source_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_signal_t:s0".into())
            .expect("invalid security context");
        let target_sid = security_server
            .security_context_to_sid(b"u:object_r:test_kill_target_t:s0".into())
            .expect("invalid security context");

        // The `signal` permission does not allow SIGKILL, SIGCHLD or SIGSTOP.
        for signal in [SIGCHLD, SIGKILL, SIGSTOP] {
            assert_eq!(
                check_signal_access(
                    &security_server.as_permission_check(),
                    source_sid,
                    target_sid,
                    signal,
                ),
                error!(EACCES)
            );
        }
    }

    #[fuchsia::test]
    fn ptrace_access_allowed_for_allowed_type_and_state_is_updated() {
        let security_server = testing::security_server_with_policy();
        let tracer_sid = security_server
            .security_context_to_sid(b"u:object_r:test_ptrace_tracer_yes_t:s0".into())
            .expect("invalid security context");
        let tracee_sid = security_server
            .security_context_to_sid(b"u:object_r:test_ptrace_traced_t:s0".into())
            .expect("invalid security context");
        let initial_state = TaskAttrs {
            current_sid: tracee_sid,
            exec_sid: None,
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: tracee_sid,
            sockcreate_sid: None,
        };
        let mut tracee_state = initial_state.clone();

        assert_eq!(
            ptrace_access_check(
                &security_server.as_permission_check(),
                tracer_sid,
                &mut tracee_state
            ),
            Ok(())
        );
        assert_eq!(
            tracee_state,
            TaskAttrs {
                current_sid: initial_state.current_sid,
                exec_sid: initial_state.exec_sid,
                fscreate_sid: initial_state.fscreate_sid,
                keycreate_sid: initial_state.keycreate_sid,
                previous_sid: initial_state.previous_sid,
                sockcreate_sid: initial_state.sockcreate_sid,
            }
        );
    }

    #[fuchsia::test]
    fn ptrace_access_denied_for_denied_type_and_state_is_not_updated() {
        let security_server = testing::security_server_with_policy();
        let tracer_sid = security_server
            .security_context_to_sid(b"u:object_r:test_ptrace_tracer_no_t:s0".into())
            .expect("invalid security context");
        let tracee_sid = security_server
            .security_context_to_sid(b"u:object_r:test_ptrace_traced_t:s0".into())
            .expect("invalid security context");
        let initial_state = TaskAttrs {
            current_sid: tracee_sid,
            exec_sid: None,
            fscreate_sid: None,
            keycreate_sid: None,
            previous_sid: tracee_sid,
            sockcreate_sid: None,
        };
        let mut tracee_state = initial_state.clone();

        assert_eq!(
            ptrace_access_check(
                &security_server.as_permission_check(),
                tracer_sid,
                &mut tracee_state
            ),
            error!(EACCES)
        );
        assert_eq!(initial_state, tracee_state);
    }

    #[fuchsia::test]
    fn task_alloc_from_parent() {
        // Create a fake parent state, with values for some fields, to check for.
        let parent_security_state = TaskAttrs {
            current_sid: SecurityId::initial(InitialSid::Unlabeled),
            previous_sid: SecurityId::initial(InitialSid::Kernel),
            exec_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
            fscreate_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
            keycreate_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
            sockcreate_sid: Some(SecurityId::initial(InitialSid::Unlabeled)),
        };

        let security_state = task_alloc(&parent_security_state, 0);
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
    async fn compute_fs_node_security_id_missing_xattr_unlabeled() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let node = &create_test_file(&mut locked, &current_task).entry.node;

        // Remove the "security.selinux" label, if any, from the test file.
        let _ = node.ops().remove_xattr(node, &current_task, XATTR_NAME_SELINUX.to_bytes().into());

        assert_eq!(
            node.ops()
                .get_xattr(node, &current_task, XATTR_NAME_SELINUX.to_bytes().into(), 4096)
                .unwrap_err(),
            errno!(ENODATA)
        );
        assert_eq!(None, testing::get_cached_sid(node));

        assert_eq!(
            SecurityId::initial(InitialSid::File),
            compute_fs_node_security_id(&security_server, &current_task, node)
        );
        assert_eq!(None, testing::get_cached_sid(node));
    }

    #[fuchsia::test]
    async fn compute_fs_node_security_id_invalid_xattr_unlabeled() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let node = &create_test_file(&mut locked, &current_task).entry.node;
        node.ops()
            .set_xattr(
                node,
                &current_task,
                XATTR_NAME_SELINUX.to_bytes().into(),
                "invalid_context!".into(),
                XattrOp::Set,
            )
            .expect("setxattr");
        assert_eq!(None, testing::get_cached_sid(node));

        assert_eq!(
            SecurityId::initial(InitialSid::Unlabeled),
            compute_fs_node_security_id(&security_server, &current_task, node)
        );
        assert_eq!(None, testing::get_cached_sid(node));
    }

    #[fuchsia::test]
    async fn compute_fs_node_security_id_valid_xattr_stored() {
        let security_server = testing::security_server_with_policy();
        security_server.set_enforcing(true);
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server.clone());
        let node = &create_test_file(&mut locked, &current_task).entry.node;
        node.ops()
            .set_xattr(
                node,
                &current_task,
                XATTR_NAME_SELINUX.to_bytes().into(),
                VALID_SECURITY_CONTEXT.into(),
                XattrOp::Set,
            )
            .expect("setxattr");
        assert_eq!(None, testing::get_cached_sid(node));

        let security_id = compute_fs_node_security_id(&security_server, &current_task, node);
        assert_eq!(Some(security_id), testing::get_cached_sid(node));
    }

    #[fuchsia::test]
    async fn setxattr_set_sid() {
        let security_server = testing::security_server_with_policy();
        let (_kernel, current_task, mut locked) =
            create_kernel_task_and_unlocked_with_selinux(security_server);
        let node = &create_test_file(&mut locked, &current_task).entry.node;
        assert_eq!(None, testing::get_cached_sid(node));

        node.set_xattr(
            current_task.as_ref(),
            &current_task.fs().root().mount,
            XATTR_NAME_SELINUX.to_bytes().into(),
            VALID_SECURITY_CONTEXT.into(),
            XattrOp::Set,
        )
        .expect("setxattr");

        assert!(testing::get_cached_sid(node).is_some());
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
