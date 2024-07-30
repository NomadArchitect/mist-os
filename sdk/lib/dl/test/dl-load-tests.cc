// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "dl-impl-tests.h"
#include "dl-system-tests.h"

// It's too much hassle the generate ELF test modules on a system where the
// host code is not usually built with ELF, so don't bother trying to test any
// of the ELF-loading logic on such hosts.  Unfortunately this means not
// discovering any <dlfcn.h> API differences from another non-ELF system that
// has that API, such as macOS.
#ifndef __ELF__
#error "This file should not be used on non-ELF hosts."
#endif

namespace {

// These are a convenience functions to specify that a specific dependency
// should or should not be found in the Needed set.
constexpr std::pair<std::string_view, bool> Found(std::string_view name) { return {name, true}; }

constexpr std::pair<std::string_view, bool> NotFound(std::string_view name) {
  return {name, false};
}

// Cast `symbol` into a function returning type T and run it.
template <typename T>
T RunFunction(void* symbol __attribute__((nonnull))) {
  auto func_ptr = reinterpret_cast<T (*)()>(reinterpret_cast<uintptr_t>(symbol));
  return func_ptr();
}

using ::testing::MatchesRegex;

template <class Fixture>
using DlTests = Fixture;

// This lists the test fixture classes to run DlTests tests against. The
// DlImplTests fixture is a framework for testing the implementation in
// libdl and the DlSystemTests fixture proxies to the system-provided dynamic
// linker. These tests ensure that both dynamic linker implementations meet
// expectations and behave the same way, with exceptions noted within the test.
using TestTypes = ::testing::Types<
#ifdef __Fuchsia__
    dl::testing::DlImplLoadZirconTests,
#endif
// TODO(https://fxbug.dev/324650368): Test fixtures currently retrieve files
// from different prefixed locations depending on the platform. Find a way
// to use a singular API to return the prefixed path specific to the platform so
// that the TestPosix fixture can run on Fuchsia as well.
#ifndef __Fuchsia__
    // libdl's POSIX test fixture can also be tested on Fuchsia and is included
    // for any ELF supported host.
    dl::testing::DlImplLoadPosixTests,
#endif
    dl::testing::DlSystemTests>;

TYPED_TEST_SUITE(DlTests, TestTypes);

TYPED_TEST(DlTests, NotFound) {
  constexpr const char* kFile = "does-not-exist.so";

  this->ExpectMissing(kFile);

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_error());
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(result.error_value().take_str(), "does-not-exist.so not found");
  } else {
    EXPECT_THAT(
        result.error_value().take_str(),
        MatchesRegex(
            // emitted by Fuchsia-musl
            "Error loading shared library .*does-not-exist.so: ZX_ERR_NOT_FOUND"
            // emitted by Linux-glibc
            "|.*does-not-exist.so: cannot open shared object file: No such file or directory"));
  }
}

TYPED_TEST(DlTests, InvalidMode) {
  constexpr const char* kFile = "ret17.module.so";

  if constexpr (!TestFixture::kCanValidateMode) {
    GTEST_SKIP() << "test requires dlopen to validate mode argment";
  }

  int bad_mode = -1;
  // The sanitizer runtimes (on non-Fuchsia hosts) intercept dlopen calls with
  // RTLD_DEEPBIND and make them fail without really calling -ldl's dlopen to
  // see if it would fail anyway.  So avoid having that flag set in the bad
  // mode argument.
#ifdef RTLD_DEEPBIND
  bad_mode &= ~RTLD_DEEPBIND;
#endif

  auto result = this->DlOpen(kFile, bad_mode);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.error_value().take_str(), "invalid mode parameter")
      << "for mode argument " << bad_mode;
}

// Load a basic file with no dependencies.
TYPED_TEST(DlTests, Basic) {
  constexpr int64_t kReturnValue = 17;
  constexpr const char* kFile = "ret17.module.so";

  // TODO(https://fxbug.dev/354043838): Move these checks from tests and into
  // the test fixture.
  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  EXPECT_TRUE(result.value());

  // Look up the "TestStart" function and call it, expecting it to return 17.
  auto sym_result = this->DlSym(result.value(), "TestStart");
  ASSERT_TRUE(sym_result.is_ok()) << result.error_value();
  ASSERT_TRUE(sym_result.value());

  EXPECT_EQ(RunFunction<int64_t>(sym_result.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(result.value()).is_ok());
}

// Load a file that performs relative relocations against itself. The TestStart
// function's return value is derived from the resolved symbols.
TYPED_TEST(DlTests, Relative) {
  constexpr int64_t kReturnValue = 17;
  constexpr const char* kFile = "relative-reloc.module.so";

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  EXPECT_TRUE(result.value());

  auto sym_result = this->DlSym(result.value(), "TestStart");
  ASSERT_TRUE(sym_result.is_ok()) << result.error_value();
  ASSERT_TRUE(sym_result.value());

  EXPECT_EQ(RunFunction<int64_t>(sym_result.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(result.value()).is_ok());
}

// Load a file that performs symbolic relocations against itself. The TestStart
// functions' return value is derived from the resolved symbols.
TYPED_TEST(DlTests, Symbolic) {
  constexpr int64_t kReturnValue = 17;
  constexpr const char* kFile = "symbolic-reloc.module.so";

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  EXPECT_TRUE(result.value());

  auto sym_result = this->DlSym(result.value(), "TestStart");
  ASSERT_TRUE(sym_result.is_ok()) << result.error_value();
  ASSERT_TRUE(sym_result.value());

  EXPECT_EQ(RunFunction<int64_t>(sym_result.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(result.value()).is_ok());
}

// Load a module that depends on a symbol provided directly by a dependency.
TYPED_TEST(DlTests, BasicDep) {
  constexpr int64_t kReturnValue = 17;
  constexpr const char* kFile = "basic-dep.module.so";
  constexpr const char* kDepFile = "libld-dep-a.so";

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);
  this->Needed({kDepFile});

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  EXPECT_TRUE(result.value());

  auto sym_result = this->DlSym(result.value(), "TestStart");
  ASSERT_TRUE(sym_result.is_ok()) << result.error_value();
  ASSERT_TRUE(sym_result.value());

  EXPECT_EQ(RunFunction<int64_t>(sym_result.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(result.value()).is_ok());
}

// Load a module that depends on a symbols provided directly and transitively by
// several dependencies. Dependency ordering is serialized such that a module
// depends on a symbol provided by a dependency only one hop away
// (e.g. in its DT_NEEDED list):
TYPED_TEST(DlTests, IndirectDeps) {
  constexpr int64_t kReturnValue = 17;
  constexpr const char* kFile = "indirect-deps.module.so";
  constexpr const char* kDepFile1 = "libindirect-deps-a.so";
  constexpr const char* kDepFile2 = "libindirect-deps-b.so";
  constexpr const char* kDepFile3 = "libindirect-deps-c.so";

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);
  this->Needed({kDepFile1, kDepFile2, kDepFile3});

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  EXPECT_TRUE(result.value());

  auto sym_result = this->DlSym(result.value(), "TestStart");
  ASSERT_TRUE(sym_result.is_ok()) << result.error_value();
  ASSERT_TRUE(sym_result.value());

  EXPECT_EQ(RunFunction<int64_t>(sym_result.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(result.value()).is_ok());
}

// Load a module that depends on symbols provided directly and transitively by
// several dependencies. Dependency ordering is DAG-like where several modules
// share a dependency.
TYPED_TEST(DlTests, ManyDeps) {
  constexpr int64_t kReturnValue = 17;
  constexpr const char* kFile = "many-deps.module.so";
  constexpr const char* kDepFile1 = "libld-dep-a.so";
  constexpr const char* kDepFile2 = "libld-dep-b.so";
  constexpr const char* kDepFile3 = "libld-dep-f.so";
  constexpr const char* kDepFile4 = "libld-dep-c.so";
  constexpr const char* kDepFile5 = "libld-dep-d.so";
  constexpr const char* kDepFile6 = "libld-dep-e.so";

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);
  this->Needed({kDepFile1, kDepFile2, kDepFile3, kDepFile4, kDepFile5, kDepFile6});

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  EXPECT_TRUE(result.value());

  auto sym_result = this->DlSym(result.value(), "TestStart");
  ASSERT_TRUE(sym_result.is_ok()) << result.error_value();
  ASSERT_TRUE(sym_result.value());

  EXPECT_EQ(RunFunction<int64_t>(sym_result.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(result.value()).is_ok());
}

// TODO(https://fxbug.dev/339028040): Test missing symbol in transitive dep.
// Load a module that depends on libld-dep-a.so, but this dependency does not
// provide the b symbol referenced by the root module, so relocation fails.
TYPED_TEST(DlTests, MissingSymbol) {
  constexpr const char* kFile = "missing-sym.module.so";
  constexpr const char* kDepFile = "libld-dep-a.so";

  this->ExpectRootModule(kFile);
  this->Needed({kDepFile});

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_error());
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(result.error_value().take_str(), "missing-sym.module.so: undefined symbol: b");
  } else {
    EXPECT_THAT(result.error_value().take_str(),
                MatchesRegex(
                    // emitted by Fuchsia-musl
                    "Error relocating missing-sym.module.so: b: symbol not found"
                    // emitted by Linux-glibc
                    "|.*missing-sym.module.so: undefined symbol: b"));
  }
}

// TODO(https://fxbug.dev/3313662773): Test simple case of transitive missing
// symbol.
// dlopen missing-transitive-symbol:
//  - missing-transitive-sym
//    - has-missing-sym is missing a()
// call a() from missing-transitive-symbol, and expect symbol not found

// Try to load a module that has a (direct) dependency that cannot be found.
TYPED_TEST(DlTests, MissingDependency) {
  constexpr const char* kFile = "missing-dep.module.so";
  constexpr const char* kDepFile = "libmissing-dep-dep.so";

  this->ExpectRootModule(kFile);
  this->Needed({NotFound(kDepFile)});

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(result.is_error());

  // TODO(https://fxbug.dev/336633049): Harmonize "not found" error messages
  // between implementations.
  // Expect that the dependency lib to missing-dep.module.so cannot be found.
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(result.error_value().take_str(), "cannot open dependency: libmissing-dep-dep.so");
  } else {
    EXPECT_THAT(
        result.error_value().take_str(),
        MatchesRegex(
            // emitted by Fuchsia-musl
            "Error loading shared library .*libmissing-dep-dep.so: ZX_ERR_NOT_FOUND \\(needed by missing-dep.module.so\\)"
            // emitted by Linux-glibc
            "|.*libmissing-dep-dep.so: cannot open shared object file: No such file or directory"));
  }
}

// Try to load a module where the dependency of its direct dependency (i.e. a
// transitive dependency of the root module) cannot be found.
TYPED_TEST(DlTests, MissingTransitiveDependency) {
  constexpr const char* kFile = "missing-transitive-dep.module.so";
  constexpr const char* kDepFile1 = "libhas-missing-dep.so";
  constexpr const char* kDepFile2 = "libmissing-dep-dep.so";

  this->ExpectRootModule(kFile);
  this->Needed({Found(kDepFile1), NotFound(kDepFile2)});

  auto result = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  // TODO(https://fxbug.dev/336633049): Harmonize "not found" error messages
  // between implementations.
  // Expect that the dependency lib to libhas-missing-dep.so cannot be found.
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(result.error_value().take_str(), "cannot open dependency: libmissing-dep-dep.so");
  } else {
    EXPECT_THAT(
        result.error_value().take_str(),
        MatchesRegex(
            // emitted by Fuchsia-musl
            "Error loading shared library .*libmissing-dep-dep.so: ZX_ERR_NOT_FOUND \\(needed by libhas-missing-dep.so\\)"
            // emitted by Linux-glibc
            "|.*libmissing-dep-dep.so: cannot open shared object file: No such file or directory"));
  }
}

// Test that calling dlopen twice on a file will return the same pointer,
// indicating that the dynamic linker is storing the module in its bookkeeping.
// dlsym() should return a pointer to the same symbol from the same module as
// well.
TYPED_TEST(DlTests, BasicModuleReuse) {
  constexpr const char* kFile = "ret17.module.so";

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile);
    }
    ASSERT_TRUE(this->DlOpen(kFile, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile);

  auto res1 = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(res1.is_ok()) << res1.error_value();
  auto ptr1 = res1.value();
  EXPECT_TRUE(ptr1);

  auto res2 = this->DlOpen(kFile, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(res2.is_ok()) << res2.error_value();
  auto ptr2 = res2.value();
  EXPECT_TRUE(ptr2);

  EXPECT_EQ(ptr1, ptr2);

  auto sym1 = this->DlSym(ptr1, "TestStart");
  ASSERT_TRUE(sym1.is_ok()) << sym1.error_value();
  auto sym1_ptr = sym1.value();
  EXPECT_TRUE(sym1_ptr);

  auto sym2 = this->DlSym(ptr2, "TestStart");
  ASSERT_TRUE(sym2.is_ok()) << sym2.error_value();
  auto sym2_ptr = sym2.value();
  EXPECT_TRUE(sym2_ptr);

  EXPECT_EQ(sym1_ptr, sym2_ptr);

  ASSERT_TRUE(this->DlClose(ptr1).is_ok());
  ASSERT_TRUE(this->DlClose(ptr2).is_ok());
}

// Test that different mutually-exclusive files that were dlopen-ed do not share
// pointers or resolved symbols.
TYPED_TEST(DlTests, UniqueModules) {
  constexpr const char* kFile1 = "ret17.module.so";
  constexpr int64_t kReturnValue17 = 17;
  constexpr const char* kFile2 = "ret23.module.so";
  constexpr int64_t kReturnValue23 = 23;

  if constexpr (TestFixture::kSupportsNoLoadMode) {
    if constexpr (TestFixture::kRetrievesFileWithNoLoad) {
      this->ExpectRootModule(kFile1);
      this->ExpectRootModule(kFile2);
    }
    ASSERT_TRUE(this->DlOpen(kFile1, RTLD_NOLOAD).is_error());
    ASSERT_TRUE(this->DlOpen(kFile2, RTLD_NOLOAD).is_error());
  }

  this->ExpectRootModule(kFile1);

  auto ret17 = this->DlOpen(kFile1, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(ret17.is_ok()) << ret17.error_value();
  auto ret17_ptr = ret17.value();
  EXPECT_TRUE(ret17_ptr);

  this->ExpectRootModule(kFile2);

  auto ret23 = this->DlOpen(kFile2, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(ret23.is_ok()) << ret23.error_value();
  auto ret23_ptr = ret23.value();
  EXPECT_TRUE(ret23_ptr);

  EXPECT_NE(ret17_ptr, ret23_ptr);

  auto sym17 = this->DlSym(ret17_ptr, "TestStart");
  ASSERT_TRUE(sym17.is_ok()) << sym17.error_value();
  auto sym17_ptr = sym17.value();
  EXPECT_TRUE(sym17_ptr);

  auto sym23 = this->DlSym(ret23_ptr, "TestStart");
  ASSERT_TRUE(sym23.is_ok()) << sym23.error_value();
  auto sym23_ptr = sym23.value();
  EXPECT_TRUE(sym23_ptr);

  EXPECT_NE(sym17_ptr, sym23_ptr);

  EXPECT_EQ(RunFunction<int64_t>(sym17_ptr), kReturnValue17);
  EXPECT_EQ(RunFunction<int64_t>(sym23_ptr), kReturnValue23);

  ASSERT_TRUE(this->DlClose(ret17_ptr).is_ok());
  ASSERT_TRUE(this->DlClose(ret23_ptr).is_ok());
}

// Test that you can dlopen a dependency from a previously loaded module.

// TODO(https://fxbug.dev/338232267): These are test scenarios that test symbol
// resolution from just the dependency graph, ie from the local scope of the
// dlopen-ed module.

// Test that dep ordering is preserved in the dependency graph.
// dlopen multiple-foo-deps -> calls foo()
//  - foo-v1 -> foo() returns 2
//  - foo-v2 -> foo() returns 7
// call foo() from multiple-foo-deps pointer and expect 2 from foo-v1.

// Test that transitive dep ordering is preserved the dependency graph.
// dlopen transitive-dep-order -> calls foo()
//   - has-foo-v1:
//     - foo-v1 -> foo() returns 2
//   - foo-v2 -> foo() returns 7
// call foo() from transitive-dep-order pointer and expect 7.

// Test that dependency ordering is always preserved in the local symbol scope,
// regardless if the dependency was already loaded.
// dlopen foo-v2 -> foo() returns 7
// dlopen multiple-foo-deps:
//   - foo-v1 -> foo() returns 2
//   - foo-v2 -> foo() returns 7
// call foo() from multiple-foo-deps and expect 2 from foo-v1 because it is
// first in multiple-foo-deps local scope.

// TODO(https://fxbug.dev/338233824): These are test scenarios that test symbol
// resolution with RTLD_GLOBAL.

// Test that a previously loaded global module symbol won't affect relative
// relocations in dlopen-ed module.
// dlopen RTLD_GLOBAL foo-v1 -> foo() returns 2
// dlopen relative-reloc-foo -> foo() returns 17
// call foo() from relative-reloc-foo and expect 17.

// Test that loaded global module will take precedence over dependency ordering.
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 7
// dlopen has-foo-v1:
//    - foo-v1 -> foo() returns 2
// call foo() from has-foo-v1 and expect foo() to return 7.

// Test that RTLD_GLOBAL applies to deps and load order will take precedence in
// subsequent symbol lookups:
// dlopen RTLD_GLOBAL has-foo-v1:
//   - foo-v1 -> foo() returns 2
// dlopen RTLD_GLOBAL has-foo-v2:
//   - foo-v2 -> foo() returns 7
// call foo from has-foo-v2 and expect 2.

// Test that missing dep will use global symbol if there's a loaded global
// module with the same symbol
// dlopen RTLD global foo-v1 -> foo() returns 2
// dlopen missing-foo:
// call foo() from missing-foo and expect 2.

}  // namespace
