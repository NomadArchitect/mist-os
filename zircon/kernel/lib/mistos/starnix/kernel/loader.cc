// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/mistos/starnix/kernel/loader.h"

#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/static-vector.h>
#include <lib/fit/result.h>
#include <lib/mistos/elfldltl/vmar-loader.h>
#include <lib/mistos/elfldltl/vmo.h>
#include <lib/mistos/starnix/kernel/mm/memory_accessor.h>
#include <lib/mistos/starnix/kernel/mm/memory_manager.h>
#include <lib/mistos/starnix/kernel/task/current_task.h>
#include <lib/mistos/starnix/kernel/task/kernel.h>
#include <lib/mistos/starnix/kernel/task/process_group.h>
#include <lib/mistos/starnix/kernel/task/task.h>
#include <lib/mistos/starnix/kernel/task/thread_group.h>
#include <lib/mistos/starnix/kernel/vfs/dir_entry.h>
#include <lib/mistos/starnix/kernel/vfs/file_object.h>
#include <lib/mistos/starnix/kernel/vfs/fs_node.h>
#include <lib/mistos/starnix_uapi/errors.h>
#include <lib/mistos/starnix_uapi/math.h>
#include <lib/mistos/util/back_insert_iterator.h>
#include <lib/mistos/util/cprng.h>
#include <lib/mistos/zx/vmo.h>
#include <lib/zbi-format/internal/bootfs.h>
#include <trace.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <cerrno>
#include <numeric>
#include <optional>

#include <fbl/ref_ptr.h>
#include <fbl/static_vector.h>
#include <fbl/string.h>
#include <fbl/vector.h>
#include <ktl/byte.h>
#include <ktl/numeric.h>
#include <ktl/span.h>

#include "../kernel_priv.h"
#include "fbl/alloc_checker.h"

#include <ktl/enforce.h>

#include <linux/auxvec.h>

#define LOCAL_TRACE STARNIX_KERNEL_GLOBAL_TRACE(0)

namespace {

using namespace starnix;

struct StackResult {
  UserAddress stack_pointer;
  UserAddress auxv_start;
  UserAddress auxv_end;
  UserAddress argv_start;
  UserAddress argv_end;
  UserAddress environ_start;
  UserAddress environ_end;
};

constexpr size_t kMaxSegments = 4;
constexpr size_t kMaxPhdrs = 16;
const size_t kRandomSeedBytes = 16;

size_t get_initial_stack_size(const fbl::String& path, const fbl::Vector<fbl::String>& argv,
                              const fbl::Vector<fbl::String>& environ,
                              const fbl::Vector<ktl::pair<uint32_t, uint64_t>>& auxv) {
  auto accumulate_size = [](size_t accumulator, const auto& arg) {
    return accumulator + arg.length() + 1;
  };

  size_t stack_size = ktl::accumulate(argv.begin(), argv.end(), 0, accumulate_size);
  stack_size += ktl::accumulate(environ.begin(), environ.end(), 0, accumulate_size);
  stack_size += path.length() + 1;
  stack_size += kRandomSeedBytes;
  stack_size += ((argv.size() + 1) + (environ.size() + 1)) * sizeof(const char*);
  stack_size += auxv.size() * 2 * sizeof(uint64_t);
  return stack_size;
}

fit::result<Errno, StackResult> populate_initial_stack(
    const MemoryAccessor& ma, UserAddress mapping_base, const fbl::String& path,
    const fbl::Vector<fbl::String>& argv, const fbl::Vector<fbl::String>& envp,
    fbl::Vector<ktl::pair<uint32_t, uint64_t>>& auxv, UserAddress original_stack_start_addr) {
  LTRACE;
  auto stack_pointer = original_stack_start_addr;

  auto write_stack = [&](const ktl::span<const uint8_t>& data,
                         UserAddress addr) -> fit::result<Errno, size_t> {
    LTRACEF("write [%lx] - %p - %zu\n", addr.ptr(), data.data(), data.size());
    return ma.write_memory(addr, data);
  };

  auto argv_end = stack_pointer;
  for (auto iter = argv.rbegin(); iter != argv.rend(); ++iter) {
    ktl::span<const uint8_t> arg{reinterpret_cast<const uint8_t*>(iter->data()),
                                 iter->length() + 1};

    stack_pointer -= arg.size();
    auto result = write_stack(arg, stack_pointer);
    if (result.is_error())
      return result.take_error();
  }
  auto argv_start = stack_pointer;

  auto environ_end = stack_pointer;
  for (auto iter = envp.rbegin(); iter != envp.rend(); ++iter) {
    ktl::span<const uint8_t> env{reinterpret_cast<const uint8_t*>(iter->data()),
                                 iter->length() + 1};
    stack_pointer -= env.size();
    auto result = write_stack(env, stack_pointer);
    if (result.is_error())
      return result.take_error();
  }
  auto environ_start = stack_pointer;

  // Write the path used with execve.
  stack_pointer -= path.length() + 1;
  auto execfn_addr = stack_pointer;
  auto result =
      write_stack({reinterpret_cast<const uint8_t*>(path.data()), path.length() + 1}, execfn_addr);
  if (result.is_error())
    return result.take_error();

  ktl::array<uint8_t, kRandomSeedBytes> random_seed{};
  cprng_draw(random_seed.data(), random_seed.size());
  stack_pointer -= random_seed.size();
  auto random_seed_addr = stack_pointer;
  result = write_stack({random_seed.data(), random_seed.size()}, random_seed_addr);
  if (result.is_error())
    return result.take_error();
  stack_pointer = random_seed_addr;

  fbl::AllocChecker ac;
  auxv.push_back(ktl::pair(AT_EXECFN, static_cast<uint64_t>(execfn_addr.ptr())), &ac);
  ZX_ASSERT(ac.check());
  auxv.push_back(ktl::pair(AT_RANDOM, static_cast<uint64_t>(random_seed_addr.ptr())), &ac);
  ZX_ASSERT(ac.check());
  auxv.push_back(ktl::pair(AT_NULL, static_cast<uint64_t>(0)), &ac);
  ZX_ASSERT(ac.check());

  // After the remainder (argc/argv/environ/auxv) is pushed, the stack pointer must be 16 byte
  // aligned. This is required by the ABI and assumed by the compiler to correctly align SSE
  // operations. But this can't be done after it's pushed, since it has to be right at the top of
  // the stack. So we collect it all, align the stack appropriately now that we know the size,
  // and push it all at once.
  fbl::Vector<uint8_t> main_data;
  // argc
  uint64_t argc = argv.size();
  ktl::span<uint8_t> argc_data(reinterpret_cast<uint8_t*>(&argc), sizeof(argc));
  ktl::copy_n(argc_data.data(), argc_data.size(), util::back_inserter(main_data));

  // argv
  constexpr fbl::static_vector<uint8_t, 8> kZero(8, 0u);
  auto next_arg_addr = argv_start;
  for (auto arg : argv) {
    ktl::span<uint8_t> ptr(reinterpret_cast<uint8_t*>(&next_arg_addr), sizeof(next_arg_addr));
    ktl::copy_n(ptr.data(), ptr.size(), util::back_inserter(main_data));
    next_arg_addr += arg.length() + 1;
  }
  ktl::copy(kZero.begin(), kZero.end(), util::back_inserter(main_data));
  // environ
  auto next_env_addr = environ_start;
  for (auto env : envp) {
    ktl::span<uint8_t> ptr(reinterpret_cast<uint8_t*>(&next_env_addr), sizeof(next_env_addr));
    ktl::copy_n(ptr.data(), ptr.size(), util::back_inserter(main_data));
    next_env_addr += env.length() + 1;
  }
  ktl::copy(kZero.begin(), kZero.end(), util::back_inserter(main_data));
  // auxv
  size_t auxv_start_offset = main_data.size();
  for (auto kv : auxv) {
    uint64_t key = static_cast<uint64_t>(kv.first);
    ktl::span<uint8_t> key_span(reinterpret_cast<uint8_t*>(&key), sizeof(key));
    ktl::span<uint8_t> value_span(reinterpret_cast<uint8_t*>(&kv.second), sizeof(kv.second));

    ktl::copy_n(key_span.data(), key_span.size(), util::back_inserter(main_data));
    ktl::copy_n(value_span.data(), value_span.size(), util::back_inserter(main_data));
  }
  size_t auxv_end_offset = main_data.size();

  // Time to push.
  stack_pointer -= main_data.size();
  stack_pointer -= stack_pointer.ptr() % 16;
  result = write_stack(main_data, stack_pointer);
  if (result.is_error())
    return result.take_error();

  auto auxv_start = stack_pointer + auxv_start_offset;
  auto auxv_end = stack_pointer + auxv_end_offset;

  return fit::ok(StackResult{
      stack_pointer,
      auxv_start,
      auxv_end,
      argv_start,
      argv_end,
      environ_start,
      environ_end,
  });
}

auto GetDiagnostics() {
  return elfldltl::Diagnostics(elfldltl::PrintfDiagnosticsReport(
                                   [](auto&&... args) {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wformat-nonliteral"
                                     if (LOCAL_TRACE)
                                       printf(args...);
#pragma GCC diagnostic pop
                                   },
                                   "resolve_elf: "),
                               elfldltl::DiagnosticsPanicFlags());
}

struct LoadedElf {
  elfldltl::Elf<>::Ehdr file_header;
  size_t file_base;
  size_t vaddr_bias;
};

fit::result<Errno, LoadedElf> load_elf(
    const FileHandle& file, zx::ArcVmo vmo,
    const fbl::RefPtr<starnix::MemoryManager>& mm /*file_write_guard: FileWriteGuardRef,*/) {
  LTRACE;
  auto diag = GetDiagnostics();
  elfldltl::UnownedVmoFile vmo_file(vmo->as_ref().borrow(), diag);
  auto headers = elfldltl::LoadHeadersFromFile<elfldltl::Elf<>>(
      diag, vmo_file, elfldltl::FixedArrayFromFile<elfldltl::Elf<>::Phdr, kMaxPhdrs>());
  ZX_ASSERT(headers);
  auto& [ehdr, phdrs_result] = *headers;
  ktl::span<const elfldltl::Elf<>::Phdr> phdrs = phdrs_result;

  // zx::vmar tmp_user_vmar;
  //{
  Guard<Mutex> lock(mm->mm_state_rw_lock());
  const auto& state = mm->state();
  // tmp_user_vmar.reset(state.user_vmar().get());
  //}
  elfldltl::StaticOrDynExecutableVmarLoader loader{state.user_vmar(),
                                                   (ehdr.type == elfldltl::ElfType::kDyn)};
  elfldltl::LoadInfo<elfldltl::Elf<>, elfldltl::StaticVector<kMaxSegments>::Container> load_info;
  ZX_ASSERT(elfldltl::DecodePhdrs(diag, phdrs, load_info.GetPhdrObserver(PAGE_SIZE)));
  ZX_ASSERT(loader.Load(diag, load_info, vmo->as_ref().borrow()));

  LTRACEF("loaded at %p, entry point %p\n", (void*)(load_info.vaddr_start() + loader.load_bias()),
          (void*)(ehdr.entry + loader.load_bias()));

  using RelroRegion = decltype(load_info)::Region;
  zx::vmar loaded_vmar = ktl::move(loader).Commit(RelroRegion{}).TakeVmar();

  return fit::ok(LoadedElf{ehdr, load_info.vaddr_start(), loader.load_bias()});
}

// Resolves a file handle into a validated executable ELF.
fit::result<Errno, starnix::ResolvedElf> resolve_elf(
    const CurrentTask& current_task, const starnix::FileHandle& file, zx::vmo vmo,
    const fbl::Vector<fbl::String>& argv, const fbl::Vector<fbl::String>& environ
    /*,selinux_state: Option<SeLinuxResolvedElfState>*/) {
  LTRACE;
  ktl::optional<starnix::ResolvedInterpElf> resolved_interp;

  auto diag = GetDiagnostics();
  elfldltl::UnownedVmoFile vmo_file(vmo.borrow(), diag);
  auto headers = elfldltl::LoadHeadersFromFile<elfldltl::Elf<>>(
      diag, vmo_file, elfldltl::FixedArrayFromFile<elfldltl::Elf<>::Phdr, kMaxPhdrs>());
  ZX_ASSERT(headers);
  auto& [ehdr, phdrs_result] = *headers;
  ktl::span<const elfldltl::Elf<>::Phdr> phdrs = phdrs_result;

  ktl::optional<elfldltl::Elf<>::Phdr> interp;
  elfldltl::LoadInfo<elfldltl::Elf<>, elfldltl::StaticVector<kMaxSegments>::Container> load_info;
  ZX_ASSERT(elfldltl::DecodePhdrs(diag, phdrs, load_info.GetPhdrObserver(PAGE_SIZE),
                                  elfldltl::PhdrInterpObserver<elfldltl::Elf<>>(interp)));

  // The ELF header specified an ELF interpreter.
  // Read the path and load this ELF as well.
  if (interp) {
    // While PT_INTERP names can be arbitrarily large, bootfs entries
    // have names of bounded length.
    constexpr size_t kInterpMaxLen = ZBI_BOOTFS_MAX_NAME_LEN;

    if (interp->filesz > kInterpMaxLen) {
      return fit::error(errno(from_status_like_fdio(ZX_ERR_INVALID_ARGS)));
    }

    // Add one for the trailing nul.
    char interp_path[kInterpMaxLen + 1];

    // Copy the suffix.
    zx_status_t status = vmo.read(interp_path, interp->offset, interp->filesz);
    if (status != ZX_OK) {
      return fit::error(errno(from_status_like_fdio(status)));
    }

    // Copy the nul.
    interp_path[interp->filesz] = '\0';

    LTRACEF("PT_INTERP %s\n", interp_path);

    auto open_result = current_task.open_file_bootfs(interp_path);
    if (open_result.is_error()) {
      return open_result.take_error();
    }

    zx::vmo interp_vmo;
    status = open_result->vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &interp_vmo);
    if (status != ZX_OK) {
      return fit::error(errno(from_status_like_fdio(status)));
    }

    fbl::AllocChecker ac;
    resolved_interp = starnix::ResolvedInterpElf{
        open_result.value(),
        ktl::move(fbl::AdoptRef(new (&ac) zx::Arc<zx::vmo>(ktl::move(interp_vmo)))),
    };
    if (!ac.check()) {
      return fit::error(errno(ENOMEM));
    }
  }

  /*
  let file_write_guard =
      file.name.entry.node.create_write_guard(FileWriteGuardMode::Exec)?.into_ref();
  */

  fbl::Vector<fbl::String> argv_cpy;
  ktl::copy(argv.begin(), argv.end(), util::back_inserter(argv_cpy));

  fbl::Vector<fbl::String> environ_cpy;
  ktl::copy(environ.begin(), environ.end(), util::back_inserter(environ_cpy));

  fbl::AllocChecker ac;
  auto arc_vmo = fbl::AdoptRef(new (&ac) zx::Arc<zx::vmo>(ktl::move(vmo)));
  if (!ac.check()) {
    return fit::error(errno(ENOMEM));
  }

  return fit::ok(starnix::ResolvedElf{
      ktl::move(file), ktl::move(arc_vmo), ktl::move(resolved_interp), ktl::move(argv_cpy),
      ktl::move(environ_cpy) /*, selinux_state, file_write_guard*/});
}

// Resolves a #! script file into a validated executable ELF.
fit::result<Errno, starnix::ResolvedElf> resolve_script(
    const starnix::CurrentTask& current_task, zx::vmo vmo, const fbl::String& path,
    const fbl::Vector<fbl::String>& argv, const fbl::Vector<fbl::String>& environ,
    size_t recursion_depth
    /*,selinux_state: Option<SeLinuxResolvedElfState>*/) {
  LTRACE;
  return fit::error(errno(-1));
}

// Resolves a file into a validated executable ELF, following script interpreters to a fixed
// recursion depth.
fit::result<Errno, starnix::ResolvedElf> resolve_executable_impl(
    const starnix::CurrentTask& current_task, const starnix::FileHandle& file, fbl::String path,
    const fbl::Vector<fbl::String>& argv, const fbl::Vector<fbl::String>& environ,
    size_t recursion_depth
    /*,selinux_state: Option<SeLinuxResolvedElfState>*/) {
  LTRACE;
  if (recursion_depth > MAX_RECURSION_DEPTH) {
    return fit::error(errno(ELOOP));
  }

  // let vmo = file.get_vmo(current_task, None, ProtectionFlags::READ | ProtectionFlags::EXEC)?;
  zx::vmo file_vmo;
  zx_status_t status = file->vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &file_vmo);
  if (status != ZX_OK) {
    return fit::error(errno(from_status_like_fdio(status)));
  }

  ktl::array<char, HASH_BANG_SIZE> header{};
  status = file_vmo.read(header.data(), 0, HASH_BANG_SIZE);
  switch (status) {
    case ZX_OK:
      break;
    case ZX_ERR_OUT_OF_RANGE:
      return fit::error(errno(ENOEXEC));
    default:
      return fit::error(errno(EINVAL));
  }

  if (header == HASH_BANG) {
    return resolve_script(current_task, ktl::move(file_vmo), path, argv, environ, recursion_depth
                          /*, selinux_state*/);
  } else {
    return resolve_elf(current_task, file, ktl::move(file_vmo), argv, environ /*, selinux_state*/);
  }
}

}  // namespace

namespace starnix {

fit::result<Errno, ResolvedElf> resolve_executable(
    const CurrentTask& current_task, const FileHandle& file, const fbl::String& path,
    const fbl::Vector<fbl::String>& argv,
    const fbl::Vector<fbl::String>& environ /*,selinux_state: Option<SeLinuxResolvedElfState>*/) {
  return resolve_executable_impl(current_task, file, path, argv, environ, 0);
}

fit::result<Errno, ThreadStartInfo> load_executable(const CurrentTask& current_task,
                                                    const ResolvedElf& resolved_elf,
                                                    const fbl::String& original_path) {
  auto main_elf = load_elf(resolved_elf.file, resolved_elf.vmo, current_task->mm()/*,
                           resolved_elf.file_write_guard*/);
  if (main_elf.is_error()) {
    return main_elf.take_error();
  }

  ktl::optional<LoadedElf> interp_elf;
  if (resolved_elf.interp.has_value()) {
    auto& interp = resolved_elf.interp.value();
    auto load_interp_result = load_elf(interp.file, interp.vmo, current_task->mm()/*,
                           resolved_elf.file_write_guard*/);
    if (load_interp_result.is_error()) {
      return load_interp_result.take_error();
    }
    interp_elf = load_interp_result.value();
  }

  auto entry_elf = interp_elf.value_or(main_elf.value());

  /*
  let entry = UserAddress::from_ptr(
        entry_elf.headers.file_header().entry.wrapping_add(entry_elf.vaddr_bias),
    );
  */
  auto entry = entry_elf.file_header.entry + entry_elf.vaddr_bias;

  LTRACEF("loaded %.*s at entry point 0x%lx\n", static_cast<int>(original_path.size()),
          original_path.data(), entry);
  /*
    let vdso_vmo = &current_task.kernel().vdso.vmo;
    let vvar_vmo = current_task.kernel().vdso.vvar_readonly.clone();

    let vdso_size = vdso_vmo.get_size().map_err(|_| errno!(EINVAL))?;
    const VDSO_PROT_FLAGS: ProtectionFlags = ProtectionFlags::READ.union(ProtectionFlags::EXEC);

    let vvar_size = vvar_vmo.get_size().map_err(|_| errno!(EINVAL))?;
    const VVAR_PROT_FLAGS: ProtectionFlags = ProtectionFlags::READ;

    // Create a private clone of the starnix kernel vDSO
    let vdso_clone = vdso_vmo
        .create_child(zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, vdso_size)
        .map_err(|status| from_status_like_fdio!(status))?;

    let vdso_executable = vdso_clone
        .replace_as_executable(&VMEX_RESOURCE)
        .map_err(|status| from_status_like_fdio!(status))?;

    // Memory map the vvar vmo, mapping a space the size of (size of vvar + size of vDSO)
    let vvar_map_result = current_task.mm().map_vmo(
        DesiredAddress::Any,
        vvar_vmo,
        0,
        (vvar_size as usize) + (vdso_size as usize),
        VVAR_PROT_FLAGS,
        MappingOptions::empty(),
        MappingName::Vvar,
        FileWriteGuardRef(None),
    )?;

    // Overwrite the second part of the vvar mapping to contain the vDSO clone
    let vdso_base = current_task.mm().map_vmo(
        DesiredAddress::FixedOverwrite(vvar_map_result + vvar_size),
        Arc::new(vdso_executable),
        0,
        vdso_size as usize,
        VDSO_PROT_FLAGS,
        MappingOptions::DONT_SPLIT,
        MappingName::Vdso,
        FileWriteGuardRef(None),
    )?;
  */

  fbl::AllocChecker ac;
  fbl::Vector<ktl::pair<uint32_t, uint64_t>> auxv;
  // auxv.push_back(ktl::make_pair(AT_UID, creds.uid));
  // auxv.push_back(ktl::make_pair(AT_EUID, creds.euid));
  // auxv.push_back(ktl::make_pair(AT_GID, creds.gid));
  // auxv.push_back(ktl::make_pair(AT_EGID, creds.egid));
  // auxv.push_back(ktl::make_pair(AT_BASE, info.has_interp ? info.interp_elf.base : 0));
  auxv.push_back(ktl::pair(AT_PAGESZ, static_cast<uint64_t>(PAGE_SIZE)), &ac);
  ZX_ASSERT(ac.check());
  // auxv.push_back(ktl::make_pair(AT_PHDR, info.main_elf.base + info.main_elf.header.phoff));
  // auxv.push_back(ktl::make_pair(AT_PHENT, info.main_elf.header.phentsize));
  // auxv.push_back(ktl::make_pair(AT_PHNUM, info.main_elf.header.phnum));
  // auxv.push_back(ktl::make_pair(AT_ENTRY, info.main_elf.load_bias + info.main_elf.header.entry));
  // auxv.push_back(ktl::make_pair(AT_SYSINFO_EHDR, vdso_base));
  auxv.push_back(ktl::pair(AT_SECURE, 0), &ac);
  ZX_ASSERT(ac.check());

  // TODO(tbodt): implement MAP_GROWSDOWN and then reset this to 1 page. The current value of
  // this is based on adding 0x1000 each time a segfault appears.
  auto stack_size_result = round_up_to_system_page_size(
      get_initial_stack_size(original_path, resolved_elf.argv, resolved_elf.environ, auxv) +
      0xf0000);

  if (stack_size_result.is_error()) {
    LTRACEF("stack is too big");
    return stack_size_result.take_error();
  }

  auto prot_flags =
      ProtectionFlags(ProtectionFlagsEnum::READ) | ProtectionFlags(ProtectionFlagsEnum::WRITE);

  auto stack_base = current_task->mm()->map_anonymous(
      {DesiredAddressType::Any, 0}, stack_size_result.value(), prot_flags,
      MappingOptionsFlags(MappingOptions::ANONYMOUS), {MappingNameType::Stack});
  if (stack_base.is_error()) {
    return stack_base.take_error();
  }

  //
  // uintptr_t sp = elfldltl::AbiTraits<>::InitialStackPointer(stack_base, stack_size);
  auto stack = stack_base.value() + (stack_size_result.value() - 8);

  auto stack_result = populate_initial_stack(current_task, stack_base.value(), original_path,
                                             resolved_elf.argv, resolved_elf.environ, auxv, stack);

  if (stack_result.is_error()) {
    return stack_result.take_error();
  }

  {
    Guard<Mutex> lock(current_task->mm()->mm_state_rw_lock());
    auto& state = current_task->mm()->state();
    state->stack_base = stack_base.value();
    state->stack_size = stack_size_result.value();
    state->stack_start = stack_result->stack_pointer;
    state->auxv_start = stack_result->auxv_start;
    state->auxv_end = stack_result->auxv_end;
    state->argv_start = stack_result->argv_start;
    state->argv_end = stack_result->argv_end;
    state->environ_start = stack_result->environ_start;
    state->environ_end = stack_result->environ_end;
    // mm_state.vdso_base = vdso_base;
  }

  return fit::ok(ThreadStartInfo{entry, stack_result->stack_pointer});
}

}  // namespace starnix
