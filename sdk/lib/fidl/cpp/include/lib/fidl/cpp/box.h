// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_BOX_H_
#define LIB_FIDL_CPP_BOX_H_

#include <zircon/assert.h>

#include <memory>

namespace fidl {

namespace internal {

template <typename... T>
constexpr bool kAlwaysFalse = false;

}  // namespace internal

// |Box<T>| is a wrapper around |std::unique_ptr<T>| that changes the behavior
// of `operator==` from pointer equality to value equality:
//
// - If one box holds a value while the other doesn't, they are not equal.
// - If both boxes don't hold values, they are equal.
// - Otherwise, delegate to `operator==` of |T|.
//
// |Box<T>| can be implicitly converted from an |std::unique_ptr<T>|.
// The primary purpose is that users will be able to use |std::unique_ptr<T>|
// in their application, and assign their data into natural domain objects
// with minimal syntax burden:
//
//     fidl::Box<T> box = std::make_unique<T>(...);
//
// |Box<T>| will be used to express optionality in the natural domain objects in
// cases where the value need to be stored on the heap to break reference cycles
// from recursively referencing types. Overall, this makes `operator==` the
// standard way to compare objects for deep equality in FIDL types.
template <typename T>
class Box : private std::unique_ptr<T> {
 public:
  constexpr Box() noexcept = default;
  ~Box() = default;
  constexpr Box(Box&& other) noexcept = default;
  constexpr Box& operator=(Box&& other) noexcept = default;

  // Support for directly assigning |unique_ptr|s into natural domain object
  // fields which are |Box|es.
  //
  // NOLINTNEXTLINE
  constexpr Box(std::unique_ptr<T> ptr) noexcept : std::unique_ptr<T>(std::move(ptr)) {}
  using std::unique_ptr<T>::unique_ptr;
  using std::unique_ptr<T>::operator=;

  // Commonly used operations from |std::unique_ptr<T>|.
  using std::unique_ptr<T>::operator*;
  using std::unique_ptr<T>::operator->;
  using std::unique_ptr<T>::operator bool;
  using std::unique_ptr<T>::reset;

  // A std::optional-like API:
  bool has_value() const { return std::unique_ptr<T>::get(); }

  const T& value() const& {
    ZX_ASSERT(has_value());
    return *std::unique_ptr<T>::get();
  }
  T& value() & {
    ZX_ASSERT(has_value());
    return *std::unique_ptr<T>::get();
  }

  template <class U>
  constexpr T value_or(U&& default_value) const& {
    if (has_value()) {
      return value();
    }
    return default_value;
  }

  // Returns the wrapped |unique_ptr|.
  std::unique_ptr<T>& unique_ptr() { return *this; }
  const std::unique_ptr<T>& unique_ptr() const { return *this; }
};

template <typename T>
constexpr bool operator==(const Box<T>& lhs, const Box<T>& rhs) noexcept {
  if (lhs) {
    if (!rhs) {
      return false;
    }
    return *lhs == *rhs;
  }
  if (rhs) {
    return false;
  }
  return true;
}

template <typename T>
constexpr bool operator!=(const Box<T>& lhs, const Box<T>& rhs) noexcept {
  return !(lhs == rhs);
}

template <typename T>
constexpr bool operator==(const std::nullptr_t& lhs, const Box<T>& rhs) noexcept {
  return rhs.unique_ptr().get() == nullptr;
}

template <typename T>
constexpr bool operator!=(const std::nullptr_t& lhs, const Box<T>& rhs) noexcept {
  return rhs.unique_ptr().get() != nullptr;
}

template <typename T>
constexpr bool operator==(const Box<T>& lhs, const std::nullptr_t& rhs) noexcept {
  return lhs.unique_ptr().get() == nullptr;
}

template <typename T>
constexpr bool operator!=(const Box<T>& lhs, const std::nullptr_t& rhs) noexcept {
  return lhs.unique_ptr().get() != nullptr;
}

template <typename T>
constexpr bool operator==(const Box<T>& lhs, const std::unique_ptr<T>& rhs) noexcept {
  static_assert(internal::kAlwaysFalse<T>,
                "Comparing a |fidl::Box<T>| and |std::unique_ptr<T>| is ambiguous. "
                "|fidl::Box<T>| implements value equality while |std::unique_ptr<T>| implements "
                "pointer equality.");
  return false;
}

template <typename T>
constexpr bool operator==(const std::unique_ptr<T>& lhs, const Box<T>& rhs) noexcept {
  static_assert(internal::kAlwaysFalse<T>,
                "Comparing a |fidl::Box<T>| and |std::unique_ptr<T>| is ambiguous. "
                "|fidl::Box<T>| implements value equality while |std::unique_ptr<T>| implements "
                "pointer equality.");
  return false;
}

template <typename T>
constexpr bool operator!=(const Box<T>& lhs, const std::unique_ptr<T>& rhs) noexcept {
  static_assert(internal::kAlwaysFalse<T>,
                "Comparing a |fidl::Box<T>| and |std::unique_ptr<T>| is ambiguous. "
                "|fidl::Box<T>| implements value equality while |std::unique_ptr<T>| implements "
                "pointer equality.");
  return false;
}

template <typename T>
constexpr bool operator!=(const std::unique_ptr<T>& lhs, const Box<T>& rhs) noexcept {
  static_assert(internal::kAlwaysFalse<T>,
                "Comparing a |fidl::Box<T>| and |std::unique_ptr<T>| is ambiguous. "
                "|fidl::Box<T>| implements value equality while |std::unique_ptr<T>| implements "
                "pointer equality.");
  return false;
}

}  // namespace fidl

#endif  // LIB_FIDL_CPP_BOX_H_
