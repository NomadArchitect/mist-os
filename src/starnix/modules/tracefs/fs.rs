// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::tracing_directory::TraceMarkerFile;
use once_cell::sync::Lazy;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    CacheMode, ConstFile, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions,
    FsNodeInfo, FsStr, StaticDirectoryBuilder,
};
use starnix_sync::{Locked, Unlocked};
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::vfs::default_statfs;
use starnix_uapi::{statfs, TRACEFS_MAGIC};

pub fn trace_fs(
    _locked: &mut Locked<'_, Unlocked>,
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    Ok(current_task
        .kernel()
        .trace_fs
        .get_or_init(|| {
            TraceFs::new_fs(current_task, options).expect("tracefs constructed with valid options")
        })
        .clone())
}

pub struct TraceFs;

impl FileSystemOps for TraceFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(TRACEFS_MAGIC))
    }

    fn name(&self) -> &'static FsStr {
        "tracefs".into()
    }
}

impl TraceFs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let fs = FileSystem::new(kernel, CacheMode::Uncached, TraceFs, options)?;
        let mut dir = StaticDirectoryBuilder::new(&fs);

        dir.node(
            "trace",
            fs.create_node(
                current_task,
                ConstFile::new_node(vec![]),
                FsNodeInfo::new_factory(mode!(IFREG, 0o755), FsCred::root()),
            ),
        );
        // The remaining contents of the fs are a minimal set of files that we want to exist so
        // that Perfetto's ftrace controller will not error out. None of them provide any real
        // functionality.
        dir.subdir(current_task, "per_cpu", 0o755, |dir| {
            /// A name for each cpu directory, cached to provide a 'static lifetime.
            static CPU_DIR_NAMES: Lazy<Vec<String>> = Lazy::new(|| {
                (0..zx::system_get_num_cpus()).map(|cpu| format!("cpu{}", cpu)).collect()
            });
            for dir_name in CPU_DIR_NAMES.iter().map(|s| s.as_str()) {
                dir.subdir(current_task, dir_name, 0o755, |dir| {
                    dir.node(
                        "trace_pipe_raw",
                        fs.create_node(
                            current_task,
                            ConstFile::new_node(vec![]),
                            FsNodeInfo::new_factory(mode!(IFREG, 0o755), FsCred::root()),
                        ),
                    );
                });
            }
        });
        dir.node(
            "tracing_on",
            fs.create_node(
                current_task,
                ConstFile::new_node("0".into()),
                FsNodeInfo::new_factory(mode!(IFREG, 0o755), FsCred::root()),
            ),
        );
        dir.node(
            "trace_marker",
            fs.create_node(
                current_task,
                TraceMarkerFile::new_node(),
                FsNodeInfo::new_factory(mode!(IFREG, 0o755), FsCred::root()),
            ),
        );
        dir.build_root();

        Ok(fs)
    }
}
