// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "starnix-loader.h"

#include <lib/mistos/starnix/kernel/mm/memory.h>
#include <lib/mistos/starnix/kernel/vfs/dir_entry.h>
#include <lib/mistos/zx/vmar.h>

namespace {

using namespace starnix;

constexpr ktl::string_view kVmoNameUnknown = "<unknown ELF file>";

constexpr ktl::string_view kVmoNamePrefixData = "data";
constexpr ktl::string_view kVmoNamePrefixBss = "bss";

constexpr char kHexDigits[] = "0132456789ABCDEF";

template <const ktl::string_view& Prefix>
void SetVmoName(zx::unowned_vmo vmo, std::string_view base_name, size_t n) {
  ktl::array<char, ZX_MAX_NAME_LEN> buffer{};
  cpp20::span vmo_name(buffer);

  // First, "data" or "bss".
  size_t name_idx = Prefix.copy(vmo_name.data(), vmo_name.size());

  // Then the ordinal in hex (almost surely just one digit, but who knows).
  // Count the bits and divide with round-up to count the nybbles.
  const size_t hex_chars = (cpp20::bit_width(n | 1) + 3) / 4;
  cpp20::span hex = cpp20::span(vmo_name).subspan(name_idx, hex_chars);
  for (auto it = hex.rbegin(); it != hex.rend(); ++it) {
    *it = kHexDigits[n & 0xf];
    n >>= 4;
  }
  name_idx += hex.size();

  // Then `:`, it's guaranteed that the worst case "dataffffffffffffffff:" (21)
  // definitely fits in ZX_MAX_NAME_LEN (32).
  vmo_name[name_idx++] = ':';

  // Finally append the original VMO name, however much fits.
  cpp20::span avail = vmo_name.subspan(name_idx);
  name_idx += base_name.copy(avail.data(), avail.size());
  ZX_DEBUG_ASSERT(name_idx <= vmo_name.size());

  vmo->set_property(ZX_PROP_NAME, vmo_name.data(), vmo_name.size());
}

}  // namespace

namespace starnix {

StarnixLoader::VmoName StarnixLoader::GetVmoName(zx::unowned_vmo vmo) {
  StarnixLoader::VmoName base_vmo_name{};
  zx_status_t status = vmo->get_property(ZX_PROP_NAME, base_vmo_name.data(), base_vmo_name.size());

  if (status != ZX_OK || base_vmo_name.front() == '\0') {
    kVmoNameUnknown.copy(base_vmo_name.data(), base_vmo_name.size());
  }

  return base_vmo_name;
}

// This has both explicit instantiations below.
template <bool ZeroInVmo>
zx_status_t StarnixLoader::MapWritable(uintptr_t vmar_offset, zx::unowned_vmo vmo, bool copy_vmo,
                                       std::string_view base_name, uint64_t vmo_offset, size_t size,
                                       size_t& num_data_segments) {
  zx::vmo writable_vmo;
  zx::unowned_vmo map_vmo;
  if (copy_vmo) {
    zx_status_t status =
        vmo->create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, vmo_offset, size, &writable_vmo);
    if (status != ZX_OK) [[unlikely]] {
      return status;
    }
    map_vmo = writable_vmo.borrow();
  } else {
    map_vmo = vmo->borrow();
  }

  // If the size is not page-aligned, zero the last page beyond the size.
  if constexpr (ZeroInVmo) {
    const size_t subpage_size = size & (page_size() - 1);
    if (subpage_size > 0) {
      const size_t zero_offset = size;
      const size_t zero_size = page_size() - subpage_size;
      zx_status_t status = map_vmo->op_range(ZX_VMO_OP_ZERO, zero_offset, zero_size, nullptr, 0);
      if (status != ZX_OK) [[unlikely]] {
        return status;
      }
    }
  }

  SetVmoName<kVmoNamePrefixData>(map_vmo->borrow(), base_name, num_data_segments++);

  return Map(vmar_offset, kMapWritable, map_vmo->borrow(), 0, size);
}

// Explicitly instantiate both flavors.

template zx_status_t StarnixLoader::MapWritable<false>(  //
    uintptr_t vmar_offset, zx::unowned_vmo vmo, bool copy_vmo, std::string_view base_name,
    uint64_t vmo_offset, size_t size, size_t& num_data_segments);

template zx_status_t StarnixLoader::MapWritable<true>(  //
    uintptr_t vmar_offset, zx::unowned_vmo vmo, bool copy_vmo, std::string_view base_name,
    uint64_t vmo_offset, size_t size, size_t& num_data_segments);

zx_status_t StarnixLoader::MapZeroFill(uintptr_t vmar_offset, ktl::string_view base_name,
                                       size_t size, size_t& num_zero_segments) {
  zx::vmo vmo;
  zx_status_t status = zx::vmo::create(size, 0, &vmo);
  if (status != ZX_OK) [[unlikely]] {
    return status;
  }

  SetVmoName<kVmoNamePrefixBss>(vmo.borrow(), base_name, num_zero_segments++);

  return Map(vmar_offset, kMapWritable, vmo.borrow(), 0, size);
}

}  // namespace starnix
