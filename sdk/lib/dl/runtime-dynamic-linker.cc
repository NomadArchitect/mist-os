// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "runtime-dynamic-linker.h"

namespace dl {

RuntimeModule* RuntimeDynamicLinker::FindModule(Soname name) {
  if (auto it = std::find(modules_.begin(), modules_.end(), name); it != modules_.end()) {
    // TODO(https://fxbug.dev/328135195): increase reference count.
    RuntimeModule& found = *it;
    return &found;
  }
  return nullptr;
}

fit::result<Error, void*> RuntimeDynamicLinker::LookupSymbol(const RuntimeModule& root,
                                                             const char* ref) {
  Diagnostics diag;
  // The root module's name is included in symbol not found errors.
  ld::ScopedModuleDiagnostics root_diag{diag, root.name().str()};

  elfldltl::SymbolName name{ref};
  // TODO(https://fxbug.dev/338229633): use elfldltl::MakeSymbolResolver.
  for (const RuntimeModule& module : root.module_tree()) {
    if (const auto* sym = name.Lookup(module.symbol_info())) {
      if (sym->type() == elfldltl::ElfSymType::kTls) {
        diag.SystemError(
            "TODO(https://fxbug.dev/331421403): TLS semantics for dlsym() are not supported yet.");
        return diag.take_error();
      }
      return diag.ok(reinterpret_cast<void*>(sym->value + module.load_bias()));
    }
  }
  diag.UndefinedSymbol(ref);
  return diag.take_error();
}

void RuntimeDynamicLinker::MakeGlobal(const ModuleTree& module_tree) {
  // This iterates through the `module_tree`, promoting any modules that are not
  // already global. When a module is promoted, it is looked up in the dynamic
  // linker's `modules_` list and moved to the back of that doubly-linked list.
  // Note, that this loop does not change the ordering of the `module_tree`.
  for (const RuntimeModule& loaded_module : module_tree) {
    // If the loaded module is already global, then its load order does not
    // change in modules_.
    if (loaded_module.is_global()) {
      continue;
    }
    // TODO(https://fxbug.dev/374810148): Introduce non-const version of ModuleTree.
    RuntimeModule& promoted = const_cast<RuntimeModule&>(loaded_module);
    promoted.set_global();
    // Move the promoted module to the back of the dynamic linker's modules_
    // list.
    modules_.push_back(modules_.erase(promoted));
  }
}

void RuntimeDynamicLinker::PopulateStartupModules(fbl::AllocChecker& func_ac,
                                                  const ld::abi::Abi<>& abi) {
  // Arm the function-level AllocChecker with the result of the function.
  auto set_result = [&func_ac](bool v) { func_ac.arm(sizeof(RuntimeModule), v); };

  for (const AbiModule& abi_module : ld::AbiLoadedModules(abi)) {
    fbl::AllocChecker ac;
    std::unique_ptr<RuntimeModule> module =
        RuntimeModule::Create(ac, Soname{abi_module.link_map.name.get()});
    if (!ac.check()) [[unlikely]] {
      set_result(false);
      return;
    }
    module->SetStartupModule(abi_module, abi);
    // TODO(https://fxbug.dev/379766260): Fill out the direct_deps of
    // startup modules.
    modules_.push_back(std::move(module));
  }

  set_result(true);
}

std::unique_ptr<RuntimeDynamicLinker> RuntimeDynamicLinker::Create(const ld::abi::Abi<>& abi,
                                                                   fbl::AllocChecker& ac) {
  assert(abi.loaded_modules);
  assert(abi.static_tls_modules.size() == abi.static_tls_offsets.size());

  // Arm the caller's AllocChecker with the return value of this function.
  auto result = [&ac](std::unique_ptr<RuntimeDynamicLinker> v) {
    ac.arm(sizeof(RuntimeDynamicLinker), static_cast<bool>(v));
    return v;
  };

  fbl::AllocChecker linker_ac;
  std::unique_ptr<RuntimeDynamicLinker> dynamic_linker{new (linker_ac) RuntimeDynamicLinker};
  if (linker_ac.check()) [[likely]] {
    fbl::AllocChecker populate_ac;
    dynamic_linker->PopulateStartupModules(populate_ac, abi);
    if (!populate_ac.check()) [[unlikely]] {
      return result(nullptr);
    }
  }

  return result(std::move(dynamic_linker));
}

}  // namespace dl
