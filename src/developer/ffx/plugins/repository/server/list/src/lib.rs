// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::api::ConfigError;
use ffx_config::EnvironmentContext;
use ffx_repository_server_list_args::ListCommand;
use fho::{
    bug, daemon_protocol, deferred, Deferred, Error, FfxMain, FfxTool, Result,
    VerifiedMachineWriter,
};
use fidl_fuchsia_developer_ffx as ffx;
use fidl_fuchsia_developer_ffx_ext::ServerStatus;
use pkg::{PkgServerInfo, PkgServerInstanceInfo, PkgServerInstances, ServerMode};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok { data: Vec<PkgServerInfo> },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}
#[derive(FfxTool)]
pub struct RepoListTool {
    #[command]
    cmd: ListCommand,
    context: EnvironmentContext,
    #[with(deferred(daemon_protocol()))]
    repos: Deferred<ffx::RepositoryRegistryProxy>,
}

fho::embedded_plugin!(RepoListTool);

#[async_trait(?Send)]
impl FfxMain for RepoListTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let full = self.cmd.full;
        let names = self.cmd.names.clone();
        match self.list().await {
            Ok(info) => {
                // filter by names
                let filtered: Vec<PkgServerInfo> = info
                    .into_iter()
                    .filter(|s| names.contains(&s.name) || names.is_empty())
                    .collect();
                writer.machine_or_else(&CommandStatus::Ok { data: filtered.clone() }, || {
                    format_text(filtered, full)
                })?;
                Ok(())
            }
            Err(e @ Error::User(_)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(e)
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

impl RepoListTool {
    async fn list(self) -> Result<Vec<PkgServerInfo>> {
        let instance_root =
            self.context.get("repository.process_dir").map_err(|e: ConfigError| bug!(e))?;
        let mgr = PkgServerInstances::new(instance_root);
        let mut instances = mgr.list_instances()?;

        // Avoid creating the daemon proxy if it is not needed.
        let has_daemon = instances.iter().any(|r| r.server_mode == ServerMode::Daemon);
        if has_daemon {
            let proxy = self.repos.await.map_err(|e| bug!(e))?;
            let status = proxy.server_status().await.map_err(|e| bug!(e))?;
            let status: ServerStatus = ServerStatus::try_from(status).map_err(|e| bug!(e))?.into();
            instances = instances
                .iter()
                .filter(|r| {
                    if r.server_mode == ServerMode::Daemon {
                        match status {
                            ServerStatus::Disabled
                            | fidl_fuchsia_developer_ffx_ext::ServerStatus::Stopped => {
                                match mgr.remove_instance(r.name.clone()) {
                                    Ok(_) => (),
                                    Err(e) => tracing::error!(
                                        "could not remove daemon instance data: {e}"
                                    ),
                                }
                                false
                            }
                            ServerStatus::Running { .. } => true,
                        }
                    } else {
                        true
                    }
                })
                .map(|r| r.clone())
                .collect();
        }
        Ok(instances)
    }
}

fn format_text(infos: Vec<PkgServerInfo>, full: bool) -> String {
    let mut lines = vec![];
    for info in infos {
        lines.push(if !full {
            format!(
                "{name: <30}\t{address}\t{repo_path}",
                name = info.name,
                address = info.address.to_string(),
                repo_path = info.repo_path
            )
        } else {
            format!(
                "{name: <30}\tpid: {pid}\n{address}\t{server_mode}\t{repo_path}\n\
            \tRegistration type: {reg_type:?}\taliases: {aliases:?}\tconflict mode: {mode:?}",
                name = info.name,
                pid = info.pid,
                address = info.address.to_string(),
                server_mode = info.server_mode,
                repo_path = info.repo_path,
                reg_type = info.registration_storage_type,
                aliases = info.registration_aliases,
                mode = info.registration_alias_conflict_mode
            )
        });
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fho::{Format, TestBuffers};
    use fidl_fuchsia_developer_ffx::RepositoryRegistryRequest;
    use futures::channel::oneshot::channel;
    use std::net::SocketAddr;
    use std::process;

    #[fuchsia::test]
    async fn test_empty() {
        let env = ffx_config::test_init().await.expect("test env");
        let fake_proxy = fho::testing::fake_proxy(move |req| panic!("Unexpected request: {req:?}"));

        let repos = Deferred::from_output(Ok(fake_proxy));

        let tool = RepoListTool {
            cmd: ListCommand { full: false, names: vec![] },
            context: env.context.clone(),
            repos,
        };
        let buffers = TestBuffers::default();
        let writer = <RepoListTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("\n", stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_text() {
        let env = ffx_config::test_init().await.expect("test env");
        let dir = env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);
        let fake_proxy = fho::testing::fake_proxy(move |req| panic!("Unexpected request: {req:?}"));

        let repos = Deferred::from_output(Ok(fake_proxy));

        let s1 = PkgServerInfo {
            name: "s1".into(),
            address: addr,
            repo_path: pkg::PathType::File("/some/repo".into()),
            registration_aliases: vec![],
            registration_storage_type: pkg::RepoStorageType::Ephemeral,
            registration_alias_conflict_mode: pkg::RegistrationConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
        };
        mgr.write_instance(&s1).expect("writing s1");

        let tool = RepoListTool {
            cmd: ListCommand { full: false, names: vec![] },
            context: env.context.clone(),
            repos,
        };
        let buffers = TestBuffers::default();
        let writer = <RepoListTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("s1                            \t[::]:8000\t/some/repo\n", stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_text_full() {
        let env = ffx_config::test_init().await.expect("test env");
        let dir = env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);
        let fake_proxy = fho::testing::fake_proxy(move |req| panic!("Unexpected request: {req:?}"));

        let repos = Deferred::from_output(Ok(fake_proxy));

        let s1 = PkgServerInfo {
            name: "s1".into(),
            address: addr,
            repo_path: pkg::PathType::File("/some/repo".into()),
            registration_aliases: vec![],
            registration_storage_type: pkg::RepoStorageType::Ephemeral,
            registration_alias_conflict_mode: pkg::RegistrationConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
        };
        mgr.write_instance(&s1).expect("writing s1");

        let tool = RepoListTool {
            cmd: ListCommand { full: true, names: vec![] },
            context: env.context.clone(),
            repos,
        };
        let buffers = TestBuffers::default();
        let writer = <RepoListTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        let pid = process::id();
        let expected = format!(
            "s1                            \tpid: {pid}\
        \n[::]:8000\tforeground\t/some/repo\
        \n\tRegistration type: Ephemeral\taliases: []\tconflict mode: ErrorOut\n"
        );
        assert_eq!(expected, stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_filter_name() {
        let env = ffx_config::test_init().await.expect("test env");
        let dir = env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);

        let (sender, _receiver) = channel();
        let mut sender = Some(sender);
        let fake_proxy = fho::testing::fake_proxy(move |req| match req {
            RepositoryRegistryRequest::ServerStatus { responder } => {
                sender.take().unwrap().send(()).unwrap();
                responder.send(&ServerStatus::Running { address: addr.clone() }.into()).unwrap()
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let repos = Deferred::from_output(Ok(fake_proxy));

        let s1 = PkgServerInfo {
            name: "s1".into(),
            address: addr,
            repo_path: pkg::PathType::File("/some/repo".into()),
            registration_aliases: vec![],
            registration_storage_type: pkg::RepoStorageType::Ephemeral,
            registration_alias_conflict_mode: pkg::RegistrationConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
        };
        mgr.write_instance(&s1).expect("writing s1");
        let s2 = PkgServerInfo {
            name: "s2".into(),
            address: addr,
            repo_path: pkg::PathType::File("/some/other/repo".into()),
            registration_aliases: vec![],
            registration_storage_type: pkg::RepoStorageType::Ephemeral,
            registration_alias_conflict_mode: pkg::RegistrationConflictMode::Replace,
            server_mode: pkg::ServerMode::Daemon,
            pid: process::id(),
        };
        mgr.write_instance(&s2).expect("writing s2");
        let tool = RepoListTool {
            cmd: ListCommand { full: false, names: vec!["s1".into()] },
            context: env.context.clone(),
            repos,
        };
        let buffers = TestBuffers::default();
        let writer = <RepoListTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("s1                            \t[::]:8000\t/some/repo\n", stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_machine_and_schema() {
        let env = ffx_config::test_init().await.expect("test env");
        let dir = env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);
        let fake_proxy = fho::testing::fake_proxy(move |req| panic!("Unexpected request: {req:?}"));

        let repos = Deferred::from_output(Ok(fake_proxy));

        let s1 = PkgServerInfo {
            name: "s1".into(),
            address: addr,
            repo_path: pkg::PathType::File("/some/repo".into()),
            registration_aliases: vec![],
            registration_storage_type: pkg::RepoStorageType::Ephemeral,
            registration_alias_conflict_mode: pkg::RegistrationConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
        };
        mgr.write_instance(&s1).expect("writing s1");

        let tool = RepoListTool {
            cmd: ListCommand { full: true, names: vec![] },
            context: env.context.clone(),
            repos,
        };
        let buffers = TestBuffers::default();
        let writer = <RepoListTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("", stderr);
        let expected = serde_json::to_string(&CommandStatus::Ok { data: vec![s1] })
            .expect("serialize expected");
        let data = serde_json::from_str(&stdout).expect("json value");

        assert_eq!(format!("{expected}\n"), stdout);
        match <RepoListTool as FfxMain>::Writer::verify_schema(&data) {
            Ok(_) => (),
            Err(e) => {
                panic!("Error verifying schema: {e} for data {data:?}");
            }
        };
    }
}
