// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/mistos/zx/vmar.h>
#include <lib/mistos/zx/vmo.h>
#include <zircon/syscalls.h>

#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

namespace {

TEST(VmoCloneTestCase, SizeAlign) {
  zx_handle_t vmo;
  zx_status_t status = zx_vmo_create(0, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // create clones with different sizes, make sure the created size is a multiple of a page size
  for (uint64_t s = 0; s < zx_system_get_page_size() * 4; s++) {
    zx_handle_t clone_vmo;
    EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SNAPSHOT, 0, s, &clone_vmo), "vm_clone");

    // should be the size rounded up to the nearest page boundary
    uint64_t size = 0x99999999;
    status = zx_vmo_get_size(clone_vmo, &size);
    EXPECT_OK(status, "vm_object_get_size");
    EXPECT_EQ(fbl::round_up(s, static_cast<size_t>(zx_system_get_page_size())), size,
              "vm_object_get_size");

    // close the handle
    EXPECT_OK(zx_handle_close(clone_vmo), "handle_close");
  }

  // close the handle
  EXPECT_OK(zx_handle_close(vmo), "handle_close");
}

// Tests that a vmo's name propagates to its child.
TEST(VmoCloneTestCase, NameProperty) {
  zx_handle_t vmo;
  zx_handle_t clone_vmo[2];

  // create a vmo
  const size_t size = zx_system_get_page_size() * 4;
  EXPECT_OK(zx_vmo_create(size, 0, &vmo), "vm_object_create");
  EXPECT_OK(zx_object_set_property(vmo, ZX_PROP_NAME, "test1", 5), "zx_object_set_property");

  // clone it
  clone_vmo[0] = ZX_HANDLE_INVALID;
  EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone_vmo[0]), "vm_clone");
  EXPECT_NE(ZX_HANDLE_INVALID, clone_vmo[0], "vm_clone_handle");
  char name[ZX_MAX_NAME_LEN];
  EXPECT_OK(zx_object_get_property(clone_vmo[0], ZX_PROP_NAME, name, ZX_MAX_NAME_LEN),
            "zx_object_get_property");
  EXPECT_TRUE(!strcmp(name, "test1"), "get_name");

  // clone it a second time w/o the rights property
  EXPECT_OK(zx_handle_replace(vmo, ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHTS_PROPERTY, &vmo));
  clone_vmo[1] = ZX_HANDLE_INVALID;
  EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone_vmo[1]), "vm_clone");
  EXPECT_NE(ZX_HANDLE_INVALID, clone_vmo[1], "vm_clone_handle");
  EXPECT_OK(zx_object_get_property(clone_vmo[0], ZX_PROP_NAME, name, ZX_MAX_NAME_LEN),
            "zx_object_get_property");
  EXPECT_TRUE(!strcmp(name, "test1"), "get_name");

  // close the original handle
  EXPECT_OK(zx_handle_close(vmo), "handle_close");

  // close the clone handles
  for (auto h : clone_vmo)
    EXPECT_OK(zx_handle_close(h), "handle_close");
}

// Returns zero on failure.
zx_rights_t GetHandleRights(zx_handle_t h) {
  zx_info_handle_basic_t info;
  zx_status_t s =
      zx_object_get_info(h, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (s != ZX_OK) {
    EXPECT_OK(s);  // Poison the test
    return 0;
  }
  return info.rights;
}

TEST(VmoCloneTestCase, Rights) {
  static const char kOldVmoName[] = "original";
  static const char kNewVmoName[] = "clone";

  static const zx_rights_t kOldVmoRights = ZX_RIGHT_READ | ZX_RIGHT_DUPLICATE;
  static const zx_rights_t kNewVmoRights =
      kOldVmoRights | ZX_RIGHT_WRITE | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY;

  zx_handle_t vmo;
  ASSERT_EQ(zx_vmo_create(zx_system_get_page_size(), 0, &vmo), ZX_OK);
  ASSERT_EQ(zx_object_set_property(vmo, ZX_PROP_NAME, kOldVmoName, sizeof(kOldVmoName)), ZX_OK);
  ASSERT_EQ(GetHandleRights(vmo) & kOldVmoRights, kOldVmoRights);

  zx_handle_t reduced_rights_vmo;
  ASSERT_EQ(zx_handle_duplicate(vmo, kOldVmoRights, &reduced_rights_vmo), ZX_OK);
  EXPECT_EQ(GetHandleRights(reduced_rights_vmo), kOldVmoRights);

  zx_handle_t clone;
  ASSERT_EQ(zx_vmo_create_child(reduced_rights_vmo, ZX_VMO_CHILD_SNAPSHOT, 0,
                                zx_system_get_page_size(), &clone),
            ZX_OK);

  EXPECT_OK(zx_handle_close(reduced_rights_vmo));

  ASSERT_EQ(zx_object_set_property(clone, ZX_PROP_NAME, kNewVmoName, sizeof(kNewVmoName)), ZX_OK);

  char oldname[ZX_MAX_NAME_LEN] = "bad";
  EXPECT_EQ(zx_object_get_property(vmo, ZX_PROP_NAME, oldname, sizeof(oldname)), ZX_OK);
  EXPECT_STREQ(oldname, kOldVmoName, "original VMO name");

  char newname[ZX_MAX_NAME_LEN] = "bad";
  EXPECT_EQ(zx_object_get_property(clone, ZX_PROP_NAME, newname, sizeof(newname)), ZX_OK);
  EXPECT_STREQ(newname, kNewVmoName, "clone VMO name");

  EXPECT_OK(zx_handle_close(vmo));
  EXPECT_EQ(GetHandleRights(clone), kNewVmoRights);
  EXPECT_OK(zx_handle_close(clone));
}

// Check that non-resizable VMOs cannot get resized.
TEST(VmoCloneTestCase, NoResize) {
  const size_t len = zx_system_get_page_size() * 4;
  zx_handle_t parent = ZX_HANDLE_INVALID;
  zx_handle_t vmo = ZX_HANDLE_INVALID;

  zx_vmo_create(len, 0, &parent);
  zx_vmo_create_child(parent, ZX_VMO_CHILD_SNAPSHOT, 0, len, &vmo);

  EXPECT_NE(vmo, ZX_HANDLE_INVALID);

  zx_status_t status;
  status = zx_vmo_set_size(vmo, len + zx_system_get_page_size());
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, status, "vm_object_set_size");

  status = zx_vmo_set_size(vmo, len - zx_system_get_page_size());
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, status, "vm_object_set_size");

  size_t size;
  status = zx_vmo_get_size(vmo, &size);
  EXPECT_OK(status, "vm_object_get_size");
  EXPECT_EQ(len, size, "vm_object_get_size");

  uintptr_t ptr;
  status = zx_vmar_map(zx_vmar_root_self(),
                       ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE, 0, vmo, 0,
                       len, &ptr);
  ASSERT_OK(status, "vm_map");
  ASSERT_NE(ptr, 0, "vm_map");

  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "unmap");

  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");

  status = zx_handle_close(parent);
  EXPECT_OK(status, "handle_close parent");
}

}  // namespace
