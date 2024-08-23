// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod tests {
    use crate::fscrypt_shared::{
        fscrypt_add_key_arg, fscrypt_key_specifier, fscrypt_remove_key_arg, FscryptOutput,
    };
    use linux_uapi::{
        fscrypt_policy_v2, FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, FSCRYPT_POLICY_FLAGS_PAD_16,
        FS_IOC_ADD_ENCRYPTION_KEY, FS_IOC_REMOVE_ENCRYPTION_KEY, FS_IOC_SET_ENCRYPTION_POLICY,
    };
    use rand::Rng;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::os::fd::AsRawFd;
    use zerocopy::{FromBytes, IntoBytes};

    const FSCRYPT_MODE_AES_256_XTS: u8 = 1;
    const FSCRYPT_MODE_AES_256_CTS: u8 = 4;

    fn add_encryption_key(root_dir: &std::fs::File) -> (i32, Vec<u8>) {
        let key_spec =
            fscrypt_key_specifier { type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, ..Default::default() };
        let arg = fscrypt_add_key_arg { key_spec: key_spec, raw_size: 64, ..Default::default() };
        let mut arg_vec = arg.as_bytes().to_vec();
        let mut random_vector: [u8; 64] = [0; 64];
        for i in 0..64 {
            let rand_u8: u8 = rand::thread_rng().gen();
            random_vector[i] = rand_u8;
        }
        arg_vec.extend(random_vector);

        let ret = unsafe {
            libc::ioctl(
                root_dir.as_raw_fd(),
                FS_IOC_ADD_ENCRYPTION_KEY.try_into().unwrap(),
                arg_vec.as_ptr(),
            )
        };
        (ret, arg_vec)
    }

    fn set_encryption_policy(dir: &std::fs::File, identifier: [u8; 16]) -> i32 {
        let ret = unsafe {
            let policy = fscrypt_policy_v2 {
                version: 2,
                contents_encryption_mode: FSCRYPT_MODE_AES_256_XTS,
                filenames_encryption_mode: FSCRYPT_MODE_AES_256_CTS,
                flags: FSCRYPT_POLICY_FLAGS_PAD_16 as u8,
                master_key_identifier: identifier,
                ..Default::default()
            };
            libc::ioctl(dir.as_raw_fd(), FS_IOC_SET_ENCRYPTION_POLICY.try_into().unwrap(), &policy)
        };
        ret
    }

    fn remove_encryption_key(root_dir: &std::fs::File, identifier: [u8; 16]) -> i32 {
        let ret = unsafe {
            let mut key_spec = fscrypt_key_specifier {
                type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER,
                ..Default::default()
            };
            key_spec.u.identifier.value = identifier;
            let remove_arg = fscrypt_remove_key_arg { key_spec: key_spec, ..Default::default() };

            libc::ioctl(
                root_dir.as_raw_fd(),
                FS_IOC_REMOVE_ENCRYPTION_KEY.try_into().unwrap(),
                &remove_arg,
            )
        };
        ret
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn remove_key_that_was_never_added() {
        let root_dir =
            std::fs::File::open(&std::env::var("MUTABLE_STORAGE").unwrap()).expect("open failed");
        let mut random_vector: [u8; 16] = [0; 16];
        for i in 0..16 {
            let rand_u8: u8 = rand::thread_rng().gen();
            random_vector[i] = rand_u8;
        }
        let ret = remove_encryption_key(&root_dir, random_vector);
        assert!(
            ret != 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn remove_key_added_by_different_user_non_root() {
        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("add encryption key failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "true",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);

        // Cleanup
        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn remove_key_added_by_different_user_root() {
        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("add encryption key failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);
        let root_dir =
            std::fs::File::open(&std::env::var("MUTABLE_STORAGE").unwrap()).expect("open failed");
        let ret = remove_encryption_key(&root_dir, fscrypt_output.identifier);

        assert!(
            ret != 0,
            "remove encryption key ioctl should have failed: {:?}",
            std::io::Error::last_os_error()
        );

        // Cleanup
        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn user_reads_directory_unlocked_by_different_user() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "set",
                "--should-fail",
                "false",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::create_dir(dir_path.join("subdir")).unwrap();

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "2000", "read", "--locked", "false"])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "2000", "read", "--locked", "true"])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn readdir_encrypted_directory_name() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::create_dir_all(dir_path.clone().join("subdir/subsubdir"))
            .expect("failed to create subdir");

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        let mut encrypted_dir_name = OsString::new();
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subdir");
            encrypted_dir_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::read_dir(dir_path.clone().join("subdir")).expect_err(
            "should not be able to readdir locked encrypted directory
                    with its plaintext filename",
        );

        let entries = std::fs::read_dir(dir_path.join(encrypted_dir_name))
            .expect("readdir of encrypted subdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subsubdir");
            count += 1;
        }
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn read_file_contents_from_handle_created_before_remove_encryption_key() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::write(dir_path.clone().join("file.txt"), "file_contents")
            .expect("create or write failed on file.txt");
        let file = std::fs::File::open(dir_path.clone().join("file.txt")).unwrap();
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };

        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let buf = std::fs::read(dir_path.join("file.txt")).expect("read file failed");
        assert_eq!(buf.as_bytes(), "file_contents".as_bytes());

        drop(file);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::read(dir_path.join("file.txt"))
            .expect_err("should not be able to read locked file");
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn readdir_locked_directory() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::create_dir(dir_path.clone().join("subdir")).expect("failed to create subdir");
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() == "subdir");
            count += 1;
        }
        assert_eq!(count, 1);

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subdir");
            count += 1;
        }
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_policy_with_fake_identifier_non_root() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let mut random_vector: [u8; 16] = [0; 16];
        for i in 0..16 {
            let rand_u8: u8 = rand::thread_rng().gen();
            random_vector[i] = rand_u8;
        }

        std::os::unix::fs::chown(dir_path.clone(), Some(2000), Some(2000)).expect("chown failed");
        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "set",
                "--should-fail",
                "true",
                "--identifier",
                &hex::encode(random_vector),
            ])
            .output()
            .expect("set encryption policy failed");

        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_policy_with_fake_identifier_root() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let mut random_vector: [u8; 16] = [0; 16];
        for i in 0..16 {
            let rand_u8: u8 = rand::thread_rng().gen();
            random_vector[i] = rand_u8;
        }
        let ret = set_encryption_policy(&dir, random_vector);
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_policy_on_directory_encrypted_with_different_policy() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct_1.key_spec.u.identifier.value) };

        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_2 = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct_2.key_spec.u.identifier.value) };

        assert!(
            ret != 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        // Cleanup
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_2.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_policy_on_file() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();

        let file =
            std::fs::File::create_new(dir_path.join("file.txt")).expect("create file failed");
        let ret = unsafe { set_encryption_policy(&file, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret != 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_policy_on_non_empty_directory() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();
        std::fs::create_dir(dir_path.join("subdir")).expect("create dir failed");
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret != 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_key_on_directory_owned_by_different_user() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        std::os::unix::fs::chown(dir_path.clone(), Some(2000), Some(2000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "set",
                "--should-fail",
                "true",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn set_encryption_policy_with_encryption_key_added_by_different_user() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        std::os::unix::fs::chown(dir_path.clone(), Some(2000), Some(2000)).expect("chown failed");
        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "set",
                "--should-fail",
                "true",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn stat_locked_file() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        {
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(dir_path.clone().join("foo.txt"))
                .unwrap();
        }

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let _ = std::fs::metadata(dir_path.clone().join("foo.txt")).expect("metadata failed");
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn one_user_adds_the_same_encryption_key_twice() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");
        let wrapping_key: [u8; 64] = [2; 64];
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        let child_binary_path = parent.join("fscrypt_test");

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn different_user_add_the_same_encryption_key() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let self_path = std::fs::read_link("/proc/self/exe").unwrap();
        let parent = self_path.parent().expect("no parent");
        let child_binary_path = parent.join("fscrypt_test");
        let wrapping_key: [u8; 64] = [2; 64];
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        eprintln!("std err is {:?}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{:#?}", output.status);
        let child_binary_path = parent.join("fscrypt_test");

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "2000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = parent.join("fscrypt_test");
        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "set",
                "--should-fail",
                "false",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);

        std::fs::create_dir(dir_path.join("subdir")).unwrap();
        drop(dir);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() == "subdir");
            count += 1;
        }
        assert_eq!(count, 1);

        let child_binary_path = parent.join("fscrypt_test");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subdir");
            count += 1;
        }
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn root_sets_encryption_policy_on_a_directory_it_does_not_own() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn unlink_locked_empty_encrypted_directory() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::create_dir(dir_path.join("subdir")).unwrap();
        drop(dir);

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut encrypted_dir_name = OsString::new();
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            encrypted_dir_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::remove_dir(dir_path.clone().join(encrypted_dir_name)).expect("remove dir failed");
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for _ in entries {
            count += 1;
        }
        assert_eq!(count, 0);
        std::fs::remove_dir(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    fn unlink_locked_encrypted_file() {
        let root_path = &std::env::var("MUTABLE_STORAGE").expect("failed to get env var");
        let root_dir = std::fs::File::open(root_path).expect("open failed");
        let dir_path = std::path::Path::new(root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        {
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(dir_path.clone().join("foo.txt"))
                .unwrap();
        }

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut encrypted_file_name = OsString::new();
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            encrypted_file_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::remove_file(dir_path.clone().join(encrypted_file_name))
            .expect("remove file failed");
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for _ in entries {
            count += 1;
        }
        assert_eq!(count, 0);
        std::fs::remove_dir(dir_path).expect("failed to remove my_dir");
    }
}
