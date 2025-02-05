// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

fn saturating_u64_to_u32(v: u64) -> u32 {
    if v > u32::max_value().into() {
        u32::max_value()
    } else {
        v as u32
    }
}

fn saturating_i64_to_i32(v: i64) -> i32 {
    if v > i32::max_value().into() {
        i32::max_value()
    } else if v < i32::min_value().into() {
        i32::min_value()
    } else {
        v as i32
    }
}

impl From<crate::stat> for crate::arch32::stat64 {
    fn from(stat: crate::stat) -> Self {
        let mut result = Self::default();
        // TODO(https://fxbug.dev/380431743): check conversions
        result.st_dev = stat.st_dev as u64;
        result.__st_ino = stat.st_ino as u32;
        result.st_mode = stat.st_mode;
        result.st_nlink = stat.st_nlink;
        result.st_uid = stat.st_uid as u32;
        result.st_gid = stat.st_gid as u32;
        result.st_rdev = stat.st_rdev;
        result.st_size = stat.st_size;
        result.st_blksize = stat.st_blksize as u32;
        result.st_blocks = stat.st_blocks as u64;
        result.st_atime = stat.st_atime as u32;
        result.st_atime_nsec = stat.st_atime_nsec as u32;
        result.st_mtime = stat.st_mtime as u32;
        result.st_mtime_nsec = stat.st_mtime_nsec as u32;
        result.st_ctime = stat.st_ctime as u32;
        result.st_ctime_nsec = stat.st_ctime_nsec as u32;
        result.st_ino = stat.st_ino;
        result
    }
}

impl From<crate::timespec> for crate::arch32::timespec {
    fn from(tv: crate::timespec) -> Self {
        Self {
            tv_sec: saturating_i64_to_i32(tv.tv_sec),
            tv_nsec: saturating_i64_to_i32(tv.tv_nsec),
        }
    }
}

impl From<crate::timeval> for crate::arch32::timeval {
    fn from(tv: crate::timeval) -> Self {
        Self {
            tv_sec: saturating_i64_to_i32(tv.tv_sec),
            tv_usec: saturating_i64_to_i32(tv.tv_usec),
        }
    }
}

impl From<crate::arch32::rlimit> for crate::rlimit {
    fn from(rlimit: crate::arch32::rlimit) -> Self {
        Self { rlim_cur: rlimit.rlim_cur.into(), rlim_max: rlimit.rlim_max.into() }
    }
}

impl From<crate::rlimit> for crate::arch32::rlimit {
    fn from(rlimit: crate::rlimit) -> Self {
        Self {
            rlim_cur: saturating_u64_to_u32(rlimit.rlim_cur),
            rlim_max: saturating_u64_to_u32(rlimit.rlim_max),
        }
    }
}

impl From<crate::arch32::sigaltstack> for crate::sigaltstack {
    fn from(sigaltstack: crate::arch32::sigaltstack) -> Self {
        Self {
            ss_sp: sigaltstack.ss_sp.into(),
            ss_flags: sigaltstack.ss_flags.into(),
            ss_size: sigaltstack.ss_size.into(),
            __bindgen_padding_0: Default::default(),
        }
    }
}

impl TryFrom<crate::sigaltstack> for crate::arch32::sigaltstack {
    type Error = ();
    fn try_from(sigaltstack: crate::sigaltstack) -> Result<Self, ()> {
        Ok(Self {
            ss_sp: sigaltstack.ss_sp.try_into().map_err(|_| ())?,
            ss_flags: sigaltstack.ss_flags.try_into().map_err(|_| ())?,
            ss_size: sigaltstack.ss_size.try_into().map_err(|_| ())?,
        })
    }
}
