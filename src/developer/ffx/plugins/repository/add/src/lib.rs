// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_repository_add_args::AddCommand;
use fho::{bug, return_user_error, user_error, FfxMain, FfxTool, Result, SimpleWriter};
use fidl_fuchsia_developer_ffx::RepositoryRegistryProxy;
use fidl_fuchsia_developer_ffx_ext::{RepositoryError, RepositorySpec};
use fuchsia_repo::repository::RepoProvider;
use fuchsia_url::RepositoryUrl;
use pkg::config as pkg_config;
use sdk_metadata::get_repositories;
use std::io::Write as _;
use target_holders::daemon_protocol;

#[derive(FfxTool)]
pub struct RepoAddTool {
    #[command]
    pub cmd: AddCommand,
    #[with(daemon_protocol())]
    repos: RepositoryRegistryProxy,
}

fho::embedded_plugin!(RepoAddTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for RepoAddTool {
    type Writer = SimpleWriter;
    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        add_from_product(self.cmd, self.repos, &mut writer).await
    }
}

pub async fn add_from_product(
    cmd: AddCommand,
    repos: RepositoryRegistryProxy,
    writer: &mut <RepoAddTool as FfxMain>::Writer,
) -> Result<()> {
    if cmd.prefix.is_empty() {
        return_user_error!("name cannot be empty");
    }
    let repositories = get_repositories(cmd.product_bundle_dir)?;
    for repository in repositories {
        // Validate that we can construct a valid repository url from the name.
        let repo_alias = repository.aliases().first().unwrap();
        let repo_url = RepositoryUrl::parse_host(format!("{}.{}", cmd.prefix, &repo_alias))
            .map_err(|err| {
                user_error!(
                    "invalid repository name for {:?} {:?}: {}",
                    cmd.prefix,
                    &repo_alias,
                    err
                )
            })?;

        let repo_name = repo_url.host();

        let repo_spec = RepositorySpec::from(repository.spec().clone()).into();

        match repos.add_repository(repo_name, &repo_spec).await.map_err(|e| bug!(e))? {
            Ok(()) => {
                // Save the filesystem configuration.
                pkg_config::set_repository(repo_name, &repository.spec())
                    .await
                    .map_err(|err| user_error!("Failed to save repository: {:#?}", err))?;

                writeln!(writer, "added repository {}", repo_name).map_err(|e| bug!(e))?;
            }
            Err(err) => {
                let err = RepositoryError::from(err);
                return_user_error!("Adding repository {} failed: {}", repo_name, err);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_partitions_config::PartitionsConfig;
    use assert_matches::assert_matches;
    use camino::Utf8Path;
    use fho::TestBuffers;
    use fidl_fuchsia_developer_ffx::{
        FileSystemRepositorySpec, RepositoryRegistryMarker, RepositoryRegistryRequest,
        RepositorySpec,
    };
    use futures::channel::mpsc;
    use futures::{SinkExt as _, StreamExt as _, TryStreamExt as _};
    use pretty_assertions::assert_eq;
    use sdk_metadata::{ProductBundle, ProductBundleV2, Repository};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_add_from_product() {
        let _test_env = ffx_config::test_init().await.expect("test initialization");
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let (mut sender, receiver) = mpsc::unbounded();

        let (repos, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<RepositoryRegistryMarker>();

        let task = fuchsia_async::Task::local(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    RepositoryRegistryRequest::AddRepository { name, repository, responder } => {
                        sender.send((name, repository)).await.unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        });

        let blobs_dir = dir.join("blobs");
        let fuchsia_metadata_dir = dir.join("fuchsia");
        let example_metadata_dir = dir.join("example");

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test-product-version".into(),
            partitions: PartitionsConfig::default(),
            sdk_version: "test-sdk-version".into(),
            system_a: None,
            system_b: None,
            system_r: None,
            repositories: vec![
                Repository {
                    name: "fuchsia.com".into(),
                    metadata_path: fuchsia_metadata_dir.clone(),
                    blobs_path: blobs_dir.clone(),
                    delivery_blob_type: 1,
                    root_private_key_path: None,
                    targets_private_key_path: None,
                    snapshot_private_key_path: None,
                    timestamp_private_key_path: None,
                },
                Repository {
                    name: "example.com".into(),
                    metadata_path: example_metadata_dir.clone(),
                    blobs_path: blobs_dir.clone(),
                    delivery_blob_type: 1,
                    root_private_key_path: None,
                    targets_private_key_path: None,
                    snapshot_private_key_path: None,
                    timestamp_private_key_path: None,
                },
            ],
            update_package_hash: None,
            virtual_devices_path: None,
        });
        pb.write(&dir).unwrap();

        let buffers = TestBuffers::default();
        let mut writer = <RepoAddTool as FfxMain>::Writer::new_test(&buffers);

        add_from_product(
            AddCommand { prefix: "my-repo".to_owned(), product_bundle_dir: dir.to_path_buf() },
            repos,
            &mut writer,
        )
        .await
        .unwrap();

        // Drop the task so the channel will close.
        drop(task);

        assert_eq!(
            receiver.collect::<Vec<_>>().await,
            vec![
                (
                    "my-repo.fuchsia.com".to_owned(),
                    RepositorySpec::FileSystem(FileSystemRepositorySpec {
                        metadata_repo_path: Some(fuchsia_metadata_dir.to_string()),
                        blob_repo_path: Some(blobs_dir.to_string()),
                        aliases: Some(vec!["fuchsia.com".into()]),
                        ..Default::default()
                    })
                ),
                (
                    "my-repo.example.com".to_owned(),
                    RepositorySpec::FileSystem(FileSystemRepositorySpec {
                        metadata_repo_path: Some(example_metadata_dir.to_string()),
                        blob_repo_path: Some(blobs_dir.to_string()),
                        aliases: Some(vec!["example.com".into()]),
                        ..Default::default()
                    })
                ),
            ]
        );
    }

    #[fuchsia::test]
    async fn test_add_from_product_rejects_invalid_names() {
        let _test_env = ffx_config::test_init().await.expect("test initialization");
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let blobs_dir = dir.join("blobs");
        let fuchsia_metadata_dir = dir.join("fuchsia");
        let example_metadata_dir = dir.join("example");

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test-product-version".into(),
            partitions: PartitionsConfig::default(),
            sdk_version: "test-sdk-version".into(),
            system_a: None,
            system_b: None,
            system_r: None,
            repositories: vec![
                Repository {
                    name: "fuchsia.com".into(),
                    metadata_path: fuchsia_metadata_dir.clone(),
                    blobs_path: blobs_dir.clone(),
                    delivery_blob_type: 1,
                    root_private_key_path: None,
                    targets_private_key_path: None,
                    snapshot_private_key_path: None,
                    timestamp_private_key_path: None,
                },
                Repository {
                    name: "example.com".into(),
                    metadata_path: example_metadata_dir.clone(),
                    blobs_path: blobs_dir.clone(),
                    delivery_blob_type: 1,
                    root_private_key_path: None,
                    targets_private_key_path: None,
                    snapshot_private_key_path: None,
                    timestamp_private_key_path: None,
                },
            ],
            update_package_hash: None,
            virtual_devices_path: None,
        });
        pb.write(&dir).unwrap();

        let buffers = TestBuffers::default();
        let mut writer = <RepoAddTool as FfxMain>::Writer::new_test(&buffers);

        let repos: RepositoryRegistryProxy = fake_proxy(move |req: RepositoryRegistryRequest| {
            panic!("should not receive any requests: {:?}", req)
        });

        for prefix in ["", "my_repo", "MyRepo", "😀"] {
            assert_matches!(
                add_from_product(
                    AddCommand { prefix: prefix.to_owned(), product_bundle_dir: dir.to_path_buf() },
                    repos.clone(),
                    &mut writer
                )
                .await,
                Err(_)
            );
        }
    }
}
