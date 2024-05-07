// Copyright 2024 Mist Tecnologia LTDA
// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/mistos/starnix/kernel/vfs/namespace.h"

#include <lib/fit/result.h>
#include <lib/mistos/starnix/kernel/task/current_task.h>
#include <lib/mistos/starnix/kernel/task/process_group.h>
#include <lib/mistos/starnix/kernel/task/task.h>
#include <lib/mistos/starnix/kernel/task/task_wrapper.h>
#include <lib/mistos/starnix/kernel/task/thread_group.h>
#include <lib/mistos/starnix/kernel/vfs/dir_entry.h>
#include <lib/mistos/starnix/kernel/vfs/file_object.h>
#include <lib/mistos/starnix/kernel/vfs/file_system.h>
#include <lib/mistos/starnix/kernel/vfs/fs_context.h>
#include <lib/mistos/starnix/kernel/vfs/fs_node.h>
#include <lib/mistos/starnix_uapi/mount_flags.h>
#include <lib/mistos/starnix_uapi/open_flags.h>
#include <trace.h>

#include <optional>
#include <utility>

#include <fbl/ref_ptr.h>

#include "../kernel_priv.h"

#define LOCAL_TRACE STARNIX_KERNEL_GLOBAL_TRACE(0)

namespace starnix {

MountFlags MountInfo::flags() {
  if (handle.has_value()) {
    return handle.value()->flags();
  } else {
    // Consider not mounted node have the NOATIME flags.
    return MountFlags(MountFlagsEnum::NOATIME);
  }
}

fit::result<Errno> MountInfo::check_readonly_filesystem() {
  if (flags().contains(MountFlagsEnum::RDONLY)) {
    return fit::error(errno(EROFS));
  }
  return fit::ok();
}

MountHandle Mount::New(WhatToMount what, MountFlags flags) {
  switch (what.type) {
    case WhatToMountEnum::Fs: {
      auto fs = std::get<FileSystemHandle>(what.what);
      return new_with_root(fs->root(), flags);
    }
    case WhatToMountEnum::Bind:
      return MountHandle();
  }
}

MountHandle Mount::new_with_root(DirEntryHandle root, MountFlags flags) {
  auto known_flags = MountFlags(MountFlagsEnum::STORED_ON_MOUNT);
  ASSERT(!flags.intersects(known_flags));

  auto fs = root->node->fs();
  auto kernel = fs->kernel().Lock();
  ASSERT_MSG(kernel, "can't create mount without a kernel");

  fbl::AllocChecker ac;
  auto handle = fbl::AdoptRef(new (&ac) Mount(kernel->get_next_mount_id(), flags, root, fs));
  ZX_ASSERT(ac.check());
  return handle;
}

Mount::Mount(uint64_t id, MountFlags flags, DirEntryHandle root, FileSystemHandle fs)
    : root_(std::move(root)), flags_(flags), fs_(std::move(fs)), id_(id) {}

NamespaceNode Mount::root() { return {{fbl::RefPtr<Mount>(this)}, root_}; }

fbl::RefPtr<Namespace> Namespace::New(FileSystemHandle fs) {
  auto kernel = fs->kernel().Lock();
  ASSERT_MSG(kernel, "can't create namespace without a kernel");

  fbl::AllocChecker ac;
  auto handle = fbl::AdoptRef(new (&ac) Namespace(
      Mount::New({WhatToMountEnum::Fs, fs}, MountFlags::empty()), kernel->get_next_namespace_id()));
  ZX_ASSERT(ac.check());
  return handle;
}

/// Create a namespace node that is not mounted in a namespace.
NamespaceNode NamespaceNode::new_anonymous(DirEntryHandle dir_entry) {
  return NamespaceNode{{}, dir_entry};
}

/// Create a namespace node that is not mounted in a namespace and that refers to a node that
/// is not rooted in a hierarchy and has no name.
NamespaceNode NamespaceNode::new_anonymous_unrooted(FsNodeHandle node) {
  return new_anonymous(DirEntry::new_unrooted(node));
}

fit::result<Errno, FileHandle> NamespaceNode::open(const CurrentTask& current_task, OpenFlags flags,
                                                   bool check_access) const {
  auto open_result = entry->node->open(current_task, mount, flags, check_access);
  if (open_result.is_error())
    return open_result.take_error();

  return FileObject::New(std::move(open_result.value()), *this, flags);
}

fit::result<Errno, NamespaceNode> NamespaceNode::open_create_node(const CurrentTask& current_task,
                                                                  const FsStr& name, FileMode mode,
                                                                  DeviceType dev, OpenFlags flags) {
  LTRACEF_LEVEL(2, "name=%s, mode=0x%x\n", name.c_str(), mode.bits());
  auto owner = current_task->as_fscred();
  auto _mode = current_task->fs()->apply_umask(mode);

  auto create_fn = [current_task, _mode, dev, owner](
                       const FsNodeHandle& dir, const MountInfo& mount,
                       const FsStr& name) -> fit::result<Errno, FsNodeHandle> {
    return dir->mknod(current_task, mount, name, _mode, dev, owner);
  };

  auto entry_result = [&]() -> fit::result<Errno, DirEntryHandle> {
    if (flags.contains(OpenFlagsEnum::EXCL)) {
      return entry->create_entry(current_task, mount, name, create_fn);
    } else {
      return entry->get_or_create_entry(current_task, mount, name, create_fn);
    }
  }();

  if (entry_result.is_error())
    return entry_result.take_error();

  return fit::ok(NamespaceNode::with_new_entry(entry_result.value()));
}

fit::result<Errno, NamespaceNode> NamespaceNode::create_node(const CurrentTask& current_task,
                                                             const FsStr& name, FileMode mode,
                                                             DeviceType dev) {
  LTRACEF_LEVEL(2, "name=%s, mode=0x%x\n", name.c_str(), mode.bits());
  auto owner = current_task->as_fscred();
  auto _mode = current_task->fs()->apply_umask(mode);
  auto result = entry->create_entry(
      current_task, mount, name,
      [current_task, _mode, dev, owner](const FsNodeHandle& dir, const MountInfo& mount,
                                        const FsStr& name) -> fit::result<Errno, FsNodeHandle> {
        return dir->mknod(current_task, mount, name, _mode, dev, owner);
      });

  if (result.is_error())
    return result.take_error();

  return fit::ok(NamespaceNode::with_new_entry(result.value()));
}

fit::result<Errno, NamespaceNode> NamespaceNode::create_tmpfile(const CurrentTask& current_task,
                                                                FileMode mode,
                                                                OpenFlags flags) const {
  // auto owner = current_task->as_fscred();
  // auto _mode = current_task->fs()->apply_umask(mode);
  return fit::error(errno(ENOTSUP));
}

fit::result<Errno, NamespaceNode> NamespaceNode::lookup_child(const CurrentTask& current_task,
                                                              LookupContext& context,
                                                              const FsStr& basename) const {
  LTRACEF_LEVEL(2, "basename=%s\n", basename.c_str());

  if (!entry->node->is_dir()) {
    return fit::error(errno(ENOTDIR));
  }

  if (basename.size() > static_cast<size_t>(NAME_MAX)) {
    return fit::error(errno(ENAMETOOLONG));
  }

  auto child_result = [&]() -> fit::result<Errno, NamespaceNode> {
    if (basename.empty() || basename == ".") {
      return fit::ok(*this);
    } else if (basename == "..") {
      NamespaceNode root;
      switch (context.resolve_base.type) {
        case None:
          root = current_task->fs()->root();
          break;
        case Beneath:
          // Do not allow traversal out of the 'node'.
          if (*this == context.resolve_base.node) {
            return fit::error(errno(EXDEV));
          }
          root = current_task->fs()->root();
          break;
        case InRoot:
          root = context.resolve_base.node;
          break;
      }

      // Make sure this can't escape a chroot.
      if (*this == root) {
        return fit::ok(root);
      } else {
        return fit::ok(parent().value_or(*this));
      }
    } else {
      auto lookup_result = entry->component_lookup(current_task, this->mount, basename);
      if (lookup_result.is_error()) {
        return lookup_result.take_error();
      }
      auto child = with_new_entry(lookup_result.value());
      while (child.entry->node->is_lnk()) {
        switch (context.symlink_mode) {
          case NoFollow:
            break;
          case Follow: {
            if ((context.remaining_follows == 0) ||
                context.resolve_flags.contains(ResolveFlagsEnum::NO_SYMLINKS)) {
              return fit::error(errno(ELOOP));
            }
            context.remaining_follows -= 1;
            auto readlink_result = child.readlink(current_task);
            if (readlink_result.is_error())
              return readlink_result.take_error();
            auto child_syslink_target = readlink_result.value();

            auto node = std::visit(
                SymlinkTarget::overloaded{
                    [&](const FsString& link_target) -> fit::result<Errno, NamespaceNode> {
                      NamespaceNode link_directory;
                      if (link_target[0] == '/') {
                        switch (context.resolve_base.type) {
                          case None:
                            link_directory = current_task->fs()->root();
                            break;
                          case Beneath:
                            return fit::error(errno(ELOOP));
                          case InRoot:
                            link_directory = context.resolve_base.node;
                            break;
                        }
                        return current_task.lookup_path(context, link_directory, link_target);

                      } else {
                        return fit::ok(*this);
                      }
                    },
                    [&](NamespaceNode node) -> fit::result<Errno, NamespaceNode> {
                      if (context.resolve_flags.contains(ResolveFlagsEnum::NO_MAGICLINKS)) {
                        return fit::error(errno(ELOOP));
                      }
                      return fit::ok(node);
                    },
                },
                readlink_result->value);
          }
        };
      }
      return fit::ok(child.enter_mount());
    }
  }();

  if (child_result.is_error())
    return child_result.take_error();
  auto child = child_result.value();

  if (context.resolve_flags.contains(ResolveFlagsEnum::NO_XDEV) &&
      child.mount.handle != mount.handle) {
    return fit::error(errno(EXDEV));
  }

  if (context.must_be_directory && !child.entry->node->is_dir()) {
    return fit::error(errno(ENOTDIR));
  }

  return fit::ok(child);
}

/// Traverse up a child-to-parent link in the namespace.
///
/// This traversal matches the child-to-parent link in the underlying
/// FsNode except at mountpoints, where the link switches from one
/// filesystem to another.
ktl::optional<NamespaceNode> NamespaceNode::parent() const { return std::nullopt; }

/// Returns the parent, but does not escape mounts i.e. returns None if this node
/// is the root of a mount.
ktl::optional<DirEntryHandle> NamespaceNode::parent_within_mount() const { return std::nullopt; }

NamespaceNode NamespaceNode::with_new_entry(DirEntryHandle _entry) const {
  return {this->mount, _entry};
}

NamespaceNode NamespaceNode::enter_mount() const {
  // While the child is a mountpoint, replace child with the mount's root.
  auto enter_one_mount = [](const NamespaceNode& node) -> ktl::optional<NamespaceNode> {
    if (auto mount_opt = node.mount.handle; mount_opt.has_value()) {
      auto mount = mount_opt.value();
      // if (mount->)
    }
    return std::nullopt;
  };

  auto inner = *this;
  while (auto some = enter_one_mount(inner)) {
    inner = some.value();
  }
  return inner;
}

fit::result<Errno, SymlinkTarget> NamespaceNode::readlink(const CurrentTask& current_task) const {
  return entry->node->readlink(current_task);
}

fit::result<Errno> NamespaceNode::check_access(const CurrentTask& current_task,
                                               Access access) const {
  return fit::ok();
}

fit::result<Errno> NamespaceNode::truncate(const CurrentTask& current_task, uint64_t length) const {
  return fit::error(errno(ENOTSUP));
}

LookupContext LookupContext::New(SymlinkMode _symlink_mode) {
  return {_symlink_mode, MAX_SYMLINK_FOLLOWS, false, ResolveFlags::empty(), {}};
}

LookupContext LookupContext::with(SymlinkMode _symlink_mode) {
  LookupContext tmp = *this;
  tmp.symlink_mode = _symlink_mode;
  tmp.resolve_base = this->resolve_base;
  return ktl::move(tmp);
}

void LookupContext::update_for_path(const FsStr& path) {
  if (path.data()[path.length()] == '/') {
    // The last path element must resolve to a directory. This is because a trailing slash
    // was found in the path.
    must_be_directory = true;
    // If the last path element is a symlink, we should follow it.
    // See https://pubs.opengroup.org/onlinepubs/9699919799/xrat/V4_xbd_chap03.html#tag_21_03_00_75
    symlink_mode = SymlinkMode::Follow;
  }
}

LookupContext LookupContext::Default() { return New(SymlinkMode::Follow); }

}  // namespace starnix
