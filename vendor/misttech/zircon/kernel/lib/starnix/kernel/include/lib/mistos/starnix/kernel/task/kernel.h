// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef VENDOR_MISTTECH_ZIRCON_KERNEL_LIB_STARNIX_KERNEL_INCLUDE_LIB_MISTOS_STARNIX_KERNEL_TASK_KERNEL_H_
#define VENDOR_MISTTECH_ZIRCON_KERNEL_LIB_STARNIX_KERNEL_INCLUDE_LIB_MISTOS_STARNIX_KERNEL_TASK_KERNEL_H_

#include <lib/expando/expando.h>
#include <lib/fit/result.h>
#include <lib/mistos/starnix/kernel/device/registry.h>
#include <lib/mistos/starnix/kernel/lifecycle/atomic_counter.h>
#include <lib/mistos/starnix/kernel/task/kernel_threads.h>
#include <lib/mistos/starnix/kernel/task/pid_table.h>
#include <lib/mistos/util/oncelock.h>
#include <lib/starnix_sync/locks.h>
#include <zircon/types.h>

#include <fbl/ref_counted_upgradeable.h>
#include <kernel/mutex.h>
#include <ktl/atomic.h>

namespace starnix {

class FileSystem;
using FileSystemHandle = fbl::RefPtr<FileSystem>;

/// Features that can be enabled for a kernel instance.
struct KernelFeatures {
  /// Whether BPF v2 is enabled.
  // bool bpf_v2 = false;

  /// Whether the kernel supports the S_ISUID and S_ISGID bits.
  ///
  /// For example, these bits are used by `sudo`.
  ///
  /// Enabling this feature is potentially a security risk because they allow privilege
  /// escalation.
  bool enable_suid = false;

  /// Whether io_uring is enabled.
  ///
  /// TODO(https://fxbug.dev/297431387): Enabled by default once the feature is completed.
  bool io_uring = false;

  /// Whether the kernel should return an error to userspace, rather than panicking, if `reboot()`
  /// is requested but cannot be enacted because the kernel lacks the relevant capabilities.
  bool error_on_failed_reboot = false;

  /// This controls whether or not the default framebuffer background is black or colorful, to
  /// aid debugging.
  // bool enable_visual_debugging = false;

  /// The default seclabel that is applied to components that are run in this kernel.
  ///
  /// Components can override this by setting the `seclabel` field in their program block.
  ktl::optional<ktl::string_view> default_seclabel;

  /// The default fsseclabel that is applied to components that are run in this kernel.
  ///
  /// Components can override this by setting the `fsseclabel` field in their program block.
  ktl::optional<ktl::string_view> default_fsseclabel;

  /// The default uid that is applied to components that are run in this kernel.
  ///
  /// Components can override this by setting the `uid` field in their program block.
  uint32_t default_uid = 0;
};

/// The shared, mutable state for the entire Starnix kernel.
///
/// The `Kernel` object holds all kernel threads, userspace tasks, and file system resources for a
/// single instance of the Starnix kernel. In production, there is one instance of this object for
/// the entire Starnix kernel. However, multiple instances of this object can be created in one
/// process during unit testing.
///
/// The structure of this object will likely need to evolve as we implement more namespacing and
/// isolation mechanisms, such as `namespaces(7)` and `pid_namespaces(7)`.
class Kernel : public fbl::RefCountedUpgradeable<Kernel> {
 public:
  /// The kernel threads running on behalf of this kernel.
  KernelThreads kthreads_;

  /// The feaures enabled for this kernel.
  KernelFeatures features_;

  // The processes and threads running in this kernel, organized by pid_t.
  starnix_sync::RwLock<PidTable> pids_;

  // Subsystem-specific properties that hang off the Kernel object.
  ///
  /// Instead of adding yet another property to the Kernel object, consider storing the property
  /// in an expando if that property is only used by one part of the system, such as a module.
  expando::Expando expando_;

  /// The default namespace for abstract AF_UNIX sockets in this kernel.
  ///
  /// Rather than use this default namespace, abstract socket addresses
  /// should be looked up in the AbstractSocketNamespace on each Task
  /// object because some Task objects might have a non-default namespace.
  // pub default_abstract_socket_namespace: Arc<AbstractUnixSocketNamespace>,

  /// The default namespace for abstract AF_VSOCK sockets in this kernel.
  // pub default_abstract_vsock_namespace: Arc<AbstractVsockSocketNamespace>,

  // The kernel command line. Shows up in /proc/cmdline.
  BString cmdline_;

  // Owned by anon_node.rs
  // pub anon_fs: OnceLock<FileSystemHandle>,
  mtl::OnceLock<FileSystemHandle> anon_fs_;

  // Owned by pipe.rs
  // pub pipe_fs: OnceLock<FileSystemHandle>,
  // Owned by socket.rs
  // pub socket_fs: OnceLock<FileSystemHandle>,
  // Owned by devtmpfs.rs
  // pub dev_tmp_fs: OnceLock<FileSystemHandle>,
  // Owned by devpts.rs
  // pub dev_pts_fs: OnceLock<FileSystemHandle>,
  // Owned by procfs.rs
  // pub proc_fs: OnceLock<FileSystemHandle>,
  // Owned by sysfs.rs
  // pub sys_fs: OnceLock<FileSystemHandle>,
  // Owned by selinux/fs.rs
  // pub selinux_fs: OnceCell<FileSystemHandle>,
  // The SELinux security server. Initialized if SELinux is enabled.
  // pub security_server: Option<Arc<SecurityServer>>,
  // Owned by tracefs/fs.rs
  // pub trace_fs: OnceLock<FileSystemHandle>,

  /// The registry of device drivers.
  DeviceRegistry device_registry_;

  /// The service directory of the container.
  // container_svc: Option<fio::DirectoryProxy>,

  /// The data directory of the container.
  // pub container_data_dir: Option<fio::DirectorySynchronousProxy>,

  /// The registry of active loop devices.
  ///
  /// See <https://man7.org/linux/man-pages/man4/loop.4.html>
  // pub loop_device_registry: Arc<LoopDeviceRegistry>,

  /// A `Framebuffer` that can be used to display a view in the workstation UI. If the container
  /// specifies the `framebuffer` feature this framebuffer will be registered as a device.
  ///
  /// When a component is run in that container and also specifies the `framebuffer` feature, the
  /// framebuffer will be served as the view of the component.
  // pub framebuffer: Arc<Framebuffer>,

  /// An `InputDevice` that can be opened to read input events from Fuchsia.
  ///
  /// If the container specifies the `framebuffer` features, this `InputDevice` will be registered
  /// as a device.
  ///
  /// When a component is run in that container, and also specifies the `framebuffer` feature,
  /// Starnix will relay input events from Fuchsia to the component.
  // pub input_device: Arc<InputDevice>,

  /// The binder driver registered for this container, indexed by their device type.
  // pub binders: RwLock<BTreeMap<DeviceType, BinderDevice>>,

  /// The iptables used for filtering network packets.
  // pub iptables: OrderedRwLock<IpTables, KernelIpTables>,

  /// The futexes shared across processes.
  // pub shared_futexes: FutexTable<SharedFutexKey>,

  /// The default UTS namespace for all tasks.
  ///
  /// Because each task can have its own UTS namespace, you probably want to use
  /// the UTS namespace handle of the task, which may/may not point to this one.
  // pub root_uts_ns: UtsNamespaceHandle,

  /// A struct containing a VMO with a vDSO implementation, if implemented for a given architecture,
  /// and possibly an offset for a sigreturn function.
  // pub vdso: Vdso,

  /// The table of devices installed on the netstack and their associated
  /// state local to this `Kernel`.
  // pub netstack_devices: Arc<NetstackDevices>,

  /// Files that are currently available for swapping.
  /// Note: Starnix never actually swaps memory to these files. We just need to track them
  /// to pass conformance tests.
  // pub swap_files: OrderedMutex<Vec<FileHandle>, KernelSwapFiles>,

  /// The implementation of generic Netlink protocol families.
  // generic_netlink: OnceCell<GenericNetlink<NetlinkToClientSender<GenericMessage>>>,

  /// The implementation of networking-related Netlink protocol families.
  // network_netlink: OnceCell<Netlink<NetlinkSenderReceiverProvider>>,

  /// Inspect instrumentation for this kernel instance.
  // inspect_node: fuchsia_inspect::Node,

  /// Diagnostics information about crashed tasks.
  // pub core_dumps: CoreDumpList,

  /// The kinds of seccomp action that gets logged, stored as a bit vector.
  /// Each potential SeccompAction gets a bit in the vector, as specified by
  /// SeccompAction::logged_bit_offset.  If the bit is set, that means the
  /// action should be logged when it is taken, subject to the caveats
  /// described in seccomp(2).  The value of the bit vector is exposed to users
  /// in a text form in the file /proc/sys/kernel/seccomp/actions_logged.
  // pub actions_logged: AtomicU16,

  /// The manager for power subsystems including reboot and suspend.
  // pub power_manager: PowerManager,

  /// Unique IDs for new mounts and mount namespaces.
  AtomicCounter<uint64_t> next_mount_id_;
  AtomicCounter<uint64_t> next_peer_group_id_;
  AtomicCounter<uint64_t> next_namespace_id_;

  /// Unique IDs for file objects.
  AtomicCounter<uint64_t> next_file_object_id_;

  /// Unique cookie used to link two inotify events, usually an IN_MOVE_FROM/IN_MOVE_TO pair.
  // pub next_inotify_cookie: AtomicU32Counter,

  /// Controls which processes a process is allowed to ptrace.  See Documentation/security/Yama.txt
  // pub ptrace_scope: AtomicU8,

  // The Fuchsia build version returned by `fuchsia.buildinfo.Provider`.
  // pub build_version: OnceCell<String>,

  // pub stats: Arc<KernelStats>,

  /// Resource limits that are exposed, for example, via sysctl.
  // pub system_limits: SystemLimits,

  // The service to handle delayed releases. This is required for elements that requires to
  // execute some code when released and requires a known context (both in term of lock context,
  // as well as `CurrentTask`).
  // DelayedReleaser delayed_releaser;

  /// Proxy to the scheduler profile provider for adjusting task priorities.
  // pub profile_provider: Option<ProfileProviderSynchronousProxy>,

  /// The syslog manager.
  // pub syslog: Syslog,

  /// All mounts.
  Mounts mounts_;

  /// impl Kernel
  static fit::result<zx_status_t, fbl::RefPtr<Kernel>> New(BString cmdline,
                                                           KernelFeatures features);

  /// impl Kernel (namespace.rs)
  uint64_t get_next_mount_id() const { return next_mount_id_.next(); }

  uint64_t get_next_peer_group_id() const { return next_peer_group_id_.next(); }

  uint64_t get_next_namespace_id() const { return next_namespace_id_.next(); }

  // C++
  ~Kernel();

 private:
  Kernel(BString cmdline, KernelFeatures features);

 public:
  mtl::WeakPtrFactory<Kernel> weak_factory_;  // must be last
};

}  // namespace starnix

#endif  // VENDOR_MISTTECH_ZIRCON_KERNEL_LIB_STARNIX_KERNEL_INCLUDE_LIB_MISTOS_STARNIX_KERNEL_TASK_KERNEL_H_
