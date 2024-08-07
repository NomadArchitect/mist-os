// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_repository_server_start_args::StartCommand;
use ffx_target::TargetProxy;
use fho::{
    daemon_protocol, return_bug, return_user_error, Connector, Error, FfxContext, FfxMain, FfxTool,
    Result, VerifiedMachineWriter,
};
use fidl_fuchsia_developer_ffx as ffx;
use fidl_fuchsia_developer_ffx_ext::RepositoryError;
use fidl_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fidl_fuchsia_net_ext::SocketAddress;
use pkg::config as pkg_config;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod server;

// The output is untagged and OK is flattened to match
// the legacy output. One day, we'll update the schema and
// worry about migration then.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok {
        #[serde(flatten)]
        address: ServerInfo,
    },
    /// Unexpected error with string.
    UnexpectedError { error_message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { error_message: String },
}

#[derive(FfxTool)]
pub struct ServerStartTool {
    #[command]
    cmd: StartCommand,
    #[with(daemon_protocol())]
    repos: ffx::RepositoryRegistryProxy,
    context: EnvironmentContext,
    pub target_proxy_connector: Connector<TargetProxy>,
    pub rcs_proxy_connector: Connector<RemoteControlProxy>,
}

fho::embedded_plugin!(ServerStartTool);

#[async_trait(?Send)]
impl FfxMain for ServerStartTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let result = match (self.cmd.daemon, self.cmd.foreground) {
            (true, false) | (false, false) => start_daemon_server(self.cmd, self.repos).await,
            (false, true) => {
                return server::run_foreground_server(
                    self.cmd,
                    self.context,
                    self.target_proxy_connector,
                    self.rcs_proxy_connector,
                    writer,
                )
                .await
            }
            (true, true) => return_user_error!("--daemon and --foreground are mutually exclusive"),
        };

        match result {
            Ok(server_addr) => {
                writer.machine_or(
                    &CommandStatus::Ok { address: ServerInfo { address: server_addr } },
                    format!("Repository server is listening on {server_addr}"),
                )?;
                Ok(())
            }
            Err(e @ Error::User(_)) => {
                writer.machine(&CommandStatus::UserError { error_message: e.to_string() })?;
                Err(e)
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { error_message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerInfo {
    address: std::net::SocketAddr,
}

async fn start_daemon_server(
    cmd: StartCommand,
    repos: ffx::RepositoryRegistryProxy,
) -> Result<std::net::SocketAddr> {
    if !pkg_config::get_repository_server_enabled().await? {
        return_user_error!("Daemon based repository serving is disabled via configuration.\n\tUse --foreground mode \
         or re-enable daemon mode by setting  repository.server.enabled to true.")
    }
    if cmd.no_device
        || !cmd.alias.is_empty()
        || cmd.port_path.is_some()
        || cmd.product_bundle.is_some()
        || cmd.repo_path.is_some()
        || cmd.repository.is_some()
        || cmd.storage_type.is_some()
        || cmd.trusted_root.is_some()
        || cmd.refresh_metadata
    {
        return_user_error!(
            "Daemon server mode does not support these options:\n\
           \t--no-device, --alias, --port-path, --product-bundle, --repo-path,\n\
           \t--repository, --storage-type, --trusted-root, --refresh-metadata"
        )
    }
    let listen_address = match {
        if let Some(addr_flag) = cmd.address {
            Ok(Some(addr_flag))
        } else {
            pkg_config::repository_listen_addr().await
        }
    } {
        Ok(Some(address)) => address,
        Ok(None) => {
            return_user_error!(
                "The server listening address is unspecified.\n\
                You can fix this by setting your ffx config.\n\
                \n\
                $ ffx config set repository.server.listen '[::]:8083'\n\
                $ ffx repository server start
                \n\
                Or alternatively specify at runtime:\n\
                $ ffx repository server start --address <IP4V_or_IP6V_addr>",
            )
        }
        Err(err) => {
            return_user_error!(
                "Failed to read repository server from ffx config or runtime flag: {:#?}",
                err
            )
        }
    };

    let runtime_address =
        if cmd.address.is_some() { Some(SocketAddress(listen_address).into()) } else { None };

    match repos
        .server_start(runtime_address.as_ref())
        .await
        .bug_context("communicating with daemon")?
        .map_err(RepositoryError::from)
    {
        Ok(address) => {
            let address = SocketAddress::from(address);

            // Error out if the server is listening on a different address. Either we raced some
            // other `start` command, or the server was already running, and someone changed the
            // `repository.server.listen` address without then stopping the server.
            if listen_address.port() != 0 && listen_address != address.0 {
                return_user_error!(
                    "The server is listening on {} but is configured to listen on {}.\n\
                    You will need to restart the server for it to listen on the\n\
                    new address. You can fix this with:\n\
                    \n\
                    $ ffx repository server stop\n\
                    $ ffx repository server start",
                    listen_address,
                    address
                )
            }

            Ok(address.0)
        }
        Err(err @ RepositoryError::ServerAddressAlreadyInUse) => {
            return_bug!("Failed to start repository server on {}: {}", listen_address, err)
        }
        Err(RepositoryError::ServerNotRunning) => {
            return_bug!(
                "Failed to start repository server on {}: {:#}",
                listen_address,
                pkg::config::determine_why_repository_server_is_not_running().await
            )
        }
        Err(err) => {
            return_bug!("Failed to start repository server on {}: {}", listen_address, err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fho::testing::ToolEnv;
    use fho::{Format, TestBuffers, TryFromEnv as _};
    use fidl_fuchsia_developer_ffx::{RepositoryError, RepositoryRegistryRequest};
    use futures::channel::oneshot::channel;
    use std::net::Ipv4Addr;

    #[fuchsia::test]
    async fn test_start_daemon() {
        let test_env = ffx_config::test_init().await.expect("test initialization");

        // enable daemon server mode in config.
        test_env
            .context
            .query("repository.server.enabled")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("true".into())
            .await
            .expect("setting repository.server.enabled");

        let address = (Ipv4Addr::LOCALHOST, 1234).into();
        test_env
            .context
            .query("repository.server.listen")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("127.0.0.1:1234".into())
            .await
            .unwrap();

        let (sender, receiver) = channel();
        let mut sender = Some(sender);
        let repos = fho::testing::fake_proxy(move |req| match req {
            RepositoryRegistryRequest::ServerStart { responder, address: None } => {
                sender.take().unwrap().send(()).unwrap();
                responder.send(Ok(&SocketAddress(address).into())).unwrap()
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        let empty_collection: TargetProxy =
            fho::testing::fake_proxy(move |_| panic!("unepxected call"));
        let tool_env = ToolEnv::new()
            .remote_factory_closure(move || async move {
                Ok(fho::testing::fake_proxy(move |_| panic!("unepxected call")))
            })
            .target_factory_closure(move || {
                let fake_target_proxy = empty_collection.clone();
                async { Ok(fake_target_proxy) }
            });

        let env = tool_env.make_environment(test_env.context.clone());
        let tool = ServerStartTool {
            cmd: StartCommand {
                daemon: true,
                foreground: false,
                address: None,
                repository: None,
                trusted_root: None,
                repo_path: None,
                product_bundle: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: ffx_repository_serve_args::default_alias_conflict_mode(),
                port_path: None,
                no_device: false,
                refresh_metadata: false,
            },
            repos,
            context: test_env.context.clone(),
            target_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make target proxy test connector"),
            rcs_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make RCS test connector"),
        };
        let buffers = TestBuffers::default();
        let writer = <ServerStartTool as FfxMain>::Writer::new_test(None, &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_ok(), "expected got {res:?} stdout == {stdout}");
        assert_eq!(stderr, "");
        assert_eq!(receiver.await, Ok(()));
    }

    #[fuchsia::test]
    async fn test_start_runtime_port() {
        let test_env = ffx_config::test_init().await.expect("test initialization");
        // enable daemon server mode in config.
        test_env
            .context
            .query("repository.server.enabled")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("true".into())
            .await
            .expect("setting repository.server.enabled");

        let address = (Ipv4Addr::LOCALHOST, 8084).into();

        let (sender, receiver) = channel();
        let mut sender = Some(sender);
        let repos = fho::testing::fake_proxy(move |req| match req {
            RepositoryRegistryRequest::ServerStart { responder, address: Some(_test) } => {
                sender.take().unwrap().send(()).unwrap();
                responder.send(Ok(&SocketAddress(address).into())).unwrap()
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        let empty_collection: TargetProxy =
            fho::testing::fake_proxy(move |_| panic!("unepxected call"));
        let tool_env = ToolEnv::new()
            .remote_factory_closure(move || async move {
                Ok(fho::testing::fake_proxy(move |_| panic!("unepxected call")))
            })
            .target_factory_closure(move || {
                let fake_target_proxy = empty_collection.clone();
                async { Ok(fake_target_proxy) }
            });

        let env = tool_env.make_environment(test_env.context.clone());
        let tool = ServerStartTool {
            cmd: StartCommand {
                daemon: true,
                foreground: false,
                address: Some("127.0.0.1:8084".parse().unwrap()),
                repository: None,
                trusted_root: None,
                repo_path: None,
                product_bundle: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: ffx_repository_serve_args::default_alias_conflict_mode(),
                port_path: None,
                no_device: false,
                refresh_metadata: false,
            },
            repos,
            context: test_env.context.clone(),
            target_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make target proxy test connector"),
            rcs_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make RCS test connector"),
        };
        let buffers = TestBuffers::default();
        let writer = <ServerStartTool as FfxMain>::Writer::new_test(None, &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_ok(), "expected ok: {stdout} {stderr}");
        assert_eq!(stderr, "");
        assert_eq!(receiver.await, Ok(()));
    }

    #[fuchsia::test]
    async fn test_start_daemon_machine() {
        let test_env = ffx_config::test_init().await.expect("test initialization");
        // enable daemon server mode in config.
        test_env
            .context
            .query("repository.server.enabled")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("true".into())
            .await
            .expect("setting repository.server.enabled");

        let address = (Ipv4Addr::LOCALHOST, 1234).into();
        test_env
            .context
            .query("repository.server.listen")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("127.0.0.1:1234".into())
            .await
            .unwrap();

        let (sender, receiver) = channel();
        let mut sender = Some(sender);
        let repos = fho::testing::fake_proxy(move |req| match req {
            RepositoryRegistryRequest::ServerStart { responder, address: None } => {
                sender.take().unwrap().send(()).unwrap();
                responder.send(Ok(&SocketAddress(address).into())).unwrap()
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        let empty_collection: TargetProxy =
            fho::testing::fake_proxy(move |_| panic!("unepxected call"));
        let tool_env = ToolEnv::new()
            .remote_factory_closure(move || async move {
                Ok(fho::testing::fake_proxy(move |_| panic!("unepxected call")))
            })
            .target_factory_closure(move || {
                let fake_target_proxy = empty_collection.clone();
                async { Ok(fake_target_proxy) }
            });

        let env = tool_env.make_environment(test_env.context.clone());
        let tool = ServerStartTool {
            cmd: StartCommand {
                daemon: true,
                foreground: false,
                address: None,
                repository: None,
                trusted_root: None,
                repo_path: None,
                product_bundle: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: ffx_repository_serve_args::default_alias_conflict_mode(),
                port_path: None,
                no_device: false,
                refresh_metadata: false,
            },
            repos,
            context: test_env.context.clone(),
            target_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make target proxy test connector"),
            rcs_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make RCS test connector"),
        };
        let buffers = TestBuffers::default();
        let writer = <ServerStartTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_ok(), "expected ok: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <ServerStartTool as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(stderr, "");
        assert_eq!(receiver.await, Ok(()));

        // Make sure the output for ok is backwards compatible with the old schema.
        assert_eq!(stdout, "{\"address\":\"127.0.0.1:1234\"}\n");
    }

    #[fuchsia::test]
    async fn test_start_failed() {
        let test_env = ffx_config::test_init().await.expect("test initialization");
        // enable daemon server mode in config.
        test_env
            .context
            .query("repository.server.enabled")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("true".into())
            .await
            .expect("setting repository.server.enabled");

        let (sender, receiver) = channel();
        let mut sender = Some(sender);
        let repos = fho::testing::fake_proxy(move |req| match req {
            RepositoryRegistryRequest::ServerStart { responder, address: None } => {
                sender.take().unwrap().send(()).unwrap();
                responder.send(Err(RepositoryError::ServerNotRunning)).unwrap()
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        let empty_collection: TargetProxy =
            fho::testing::fake_proxy(move |_| panic!("unepxected call"));

        let tool_env = ToolEnv::new()
            .remote_factory_closure(move || async move {
                Ok(fho::testing::fake_proxy(move |_| panic!("unepxected call")))
            })
            .target_factory_closure(move || {
                let fake_target_proxy = empty_collection.clone();
                async { Ok(fake_target_proxy) }
            });

        let env = tool_env.make_environment(test_env.context.clone());

        let tool = ServerStartTool {
            cmd: StartCommand {
                daemon: true,
                foreground: false,
                address: None,
                repository: None,
                trusted_root: None,
                repo_path: None,
                product_bundle: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: ffx_repository_serve_args::default_alias_conflict_mode(),
                port_path: None,
                no_device: false,
                refresh_metadata: false,
            },
            repos,
            context: test_env.context.clone(),
            target_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make target proxy test connector"),
            rcs_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make RCS test connector"),
        };
        let buffers = TestBuffers::default();
        let writer = <ServerStartTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_err(), "expected err: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <ServerStartTool as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(stderr, "");
        assert_eq!(receiver.await, Ok(()));
    }

    #[fuchsia::test]
    async fn test_start_wrong_port() {
        let test_env = ffx_config::test_init().await.expect("test initialization");
        // enable daemon server mode in config.
        test_env
            .context
            .query("repository.server.enabled")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("true".into())
            .await
            .expect("setting repository.server.enabled");

        let address = (Ipv4Addr::LOCALHOST, 1234).into();
        test_env
            .context
            .query("repository.server.listen")
            .level(Some(ffx_config::ConfigLevel::User))
            .set("127.0.0.1:4321".into())
            .await
            .unwrap();

        let (sender, receiver) = channel();
        let mut sender = Some(sender);
        let repos = fho::testing::fake_proxy(move |req| match req {
            RepositoryRegistryRequest::ServerStart { responder, address: None } => {
                sender.take().unwrap().send(()).unwrap();
                responder.send(Ok(&SocketAddress(address).into())).unwrap()
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        let empty_collection: TargetProxy =
            fho::testing::fake_proxy(move |_| panic!("unepxected call"));

        let tool_env = ToolEnv::new()
            .remote_factory_closure(move || async move {
                Ok(fho::testing::fake_proxy(move |_| panic!("unepxected call")))
            })
            .target_factory_closure(move || {
                let fake_target_proxy = empty_collection.clone();
                async { Ok(fake_target_proxy) }
            });

        let env = tool_env.make_environment(test_env.context.clone());

        let tool = ServerStartTool {
            cmd: StartCommand {
                daemon: true,
                foreground: false,
                address: None,
                repository: None,
                trusted_root: None,
                repo_path: None,
                product_bundle: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: ffx_repository_serve_args::default_alias_conflict_mode(),
                port_path: None,
                no_device: false,
                refresh_metadata: false,
            },
            repos,
            context: env.context.clone(),
            target_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make target proxy test connector"),
            rcs_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make RCS test connector"),
        };

        let buffers = TestBuffers::default();
        let writer = <ServerStartTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_err(), "expected err: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <ServerStartTool as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(stderr, "");
        assert_eq!(receiver.await, Ok(()));
    }
}
