// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::system_get_page_size;

/// Returns the starting address of the page that contains this address. For example, if page size
/// is 0x1000, page_start(0x3001) == page_start(0x3FAB) == 0x3000.
pub fn page_start(addr: usize) -> usize {
    addr & !(system_get_page_size() as usize - 1)
}

/// Returns the offset of the address within its page. For example, if page size is 0x1000,
/// page_offset(0x2ABC) == page_offset(0x5ABC) == 0xABC.
pub fn page_offset(addr: usize) -> usize {
    addr & (system_get_page_size() as usize - 1)
}

/// Returns starting address of the next page after the one that contains this address, unless
/// address is already page aligned. For example, if page size is 0x1000, page_end(0x4001) ==
/// page_end(0x4FFF) == 0x5000, but page_end(0x4000) == 0x4000.
pub fn page_end(addr: usize) -> usize {
    page_start(addr + (system_get_page_size() as usize) - 1)
}
