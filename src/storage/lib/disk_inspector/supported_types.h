// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_DISK_INSPECTOR_SUPPORTED_TYPES_H_
#define SRC_STORAGE_LIB_DISK_INSPECTOR_SUPPORTED_TYPES_H_

namespace disk_inspector {

// Enum listing the types that DiskStruct is able to parse. This is necessary
// because we currently do not have runtime type information support to use
// typeid and type_info.
enum class FieldType {
  kNotSupported,
  kUint8,
  kUint16,
  kUint32,
  kUint64,
  kDiskStruct,
};

// Options controlling how to display a DiskStruct/DiskPrimitive as a string.
struct PrintOptions {
  bool display_hex;
  bool hide_array;
};

}  // namespace disk_inspector

#endif  // SRC_STORAGE_LIB_DISK_INSPECTOR_SUPPORTED_TYPES_H_
