// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Error;
use fidl_fuchsia_tee::ApplicationRequestStream;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_dir_root};
use fuchsia_component::server::ServiceFs;
use fuchsia_tee_manager_config::TAConfig;
use futures::prelude::*;
use tee_internal::binding::TEE_ERROR_TARGET_DEAD;
use vfs::file::vmo::read_only;
use vfs::file::File;
use {fidl_fuchsia_io as fio, fuchsia_async as fasync};

struct TAConnectRequest {
    uuid: String,
    stream: ApplicationRequestStream,
}

async fn run_application(mut request: TAConnectRequest, config: TAConfig) {
    // TODO: Check the config to see if this is singleInstance. If so, look for an existing
    // connection to this TA and connect to it.

    let child_name = request.uuid.to_string(); // TODO: This won't work for multi instance TAs
    let realm = connect_to_protocol::<fidl_fuchsia_component::RealmMarker>()
        .expect("connecting to Realm protocol");
    let child_decl = fidl_fuchsia_component_decl::Child {
        name: Some(child_name.to_string()),
        url: Some(config.url),
        startup: Some(fidl_fuchsia_component_decl::StartupMode::Eager),
        ..Default::default()
    };
    let (child_controller, child_controller_server) =
        fidl::endpoints::create_proxy().expect("creating child controller channel");
    let create_child_args = fidl_fuchsia_component::CreateChildArgs {
        controller: Some(child_controller_server),
        ..Default::default()
    };
    if let Err(e) = realm
        .create_child(
            &fidl_fuchsia_component_decl::CollectionRef { name: "ta".to_string() },
            &child_decl,
            create_child_args,
        )
        .await
    {
        tracing::error!("Could not create child component in TA collection: {e:?}");
        return;
    }

    let (ta_exposed_dir, ta_exposed_dir_server) =
        fidl::endpoints::create_proxy().expect("creating exposed directory channel");
    if let Err(e) = realm
        .open_exposed_dir(
            &fidl_fuchsia_component_decl::ChildRef {
                name: child_name,
                collection: Some("ta".to_string()),
            },
            ta_exposed_dir_server,
        )
        .await
    {
        tracing::error!("Could not open exposed directory on child component: {e:?}");
        return;
    }

    let ta =
        connect_to_protocol_at_dir_root::<fidl_fuchsia_tee::ApplicationMarker>(&ta_exposed_dir)
            .expect("connecting to Application protocol on child TA");

    use fidl_fuchsia_tee as ftee;
    fn overwrite_return_origin(mut result: ftee::OpResult) -> ftee::OpResult {
        result.return_origin = Some(ftee::ReturnOrigin::TrustedApplication);
        result
    }

    while let Some(request) = request.stream.next().await {
        match request {
            Ok(ftee::ApplicationRequest::OpenSession2 { parameter_set, responder }) => {
                let _ = match ta.open_session2(parameter_set).await {
                    Ok((code, result)) => responder.send(code, overwrite_return_origin(result)),
                    Err(_) => responder.send(
                        0,
                        ftee::OpResult {
                            return_code: Some(TEE_ERROR_TARGET_DEAD as u64),
                            return_origin: Some(ftee::ReturnOrigin::TrustedOs),
                            ..Default::default()
                        },
                    ),
                };
            }
            Ok(ftee::ApplicationRequest::InvokeCommand {
                session_id,
                command_id,
                parameter_set,
                responder,
            }) => {
                let _ = match ta.invoke_command(session_id, command_id, parameter_set).await {
                    Ok(result) => responder.send(overwrite_return_origin(result)),
                    Err(_) => responder.send(ftee::OpResult {
                        return_code: Some(TEE_ERROR_TARGET_DEAD as u64),
                        return_origin: Some(ftee::ReturnOrigin::TrustedOs),
                        ..Default::default()
                    }),
                };
            }
            Ok(ftee::ApplicationRequest::CloseSession { session_id, responder }) => {
                // This always succeeds - even if the TA isn't there we report success.
                let _ = ta.close_session(session_id).await;
                let _ = responder.send();
            }
            Err(_) => break,
        }
    }
    // TODO: We may need to keep the instance alive when we implement the singleInstance and
    // instanceKeepAlive properties. For now, explicitly drop the child controller when the client
    // disconnects.
    std::mem::drop(child_controller);
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut configs = HashMap::new();
    if let Ok(d) = std::fs::read_dir("/config") {
        let uuid_filenames = d
            .filter(|entry| entry.is_ok())
            .map(|entry| entry.unwrap().path())
            .filter(|path| !path.is_dir())
            .map(|entry| (entry.clone(), entry.file_name().unwrap().to_os_string()))
            .collect::<Vec<_>>();
        for (path, file) in uuid_filenames {
            let uuid = Path::new(&file)
                .file_stem()
                .ok_or(anyhow::anyhow!("Expected path with extension"))?;
            let config = TAConfig::parse_config(&path)?;
            let _ = configs.insert(
                uuid.to_str().ok_or(anyhow::anyhow!("UUID string did not decode"))?.to_string(),
                config,
            );
        }
    }

    let mut fs = ServiceFs::new_local();
    let mut svc_dir = fs.dir("svc");
    let mut ta_dir = svc_dir.dir("ta");

    for uuid in configs.keys().cloned() {
        let _ = ta_dir.dir(&uuid).add_fidl_service(move |stream: ApplicationRequestStream| {
            TAConnectRequest { uuid: uuid.to_string(), stream }
        });
    }

    let system_props =
        std::fs::read_to_string(std::path::Path::new("/pkg/data/properties/system_properties"))?;
    let mut data_dir = fs.dir("data");
    let mut properties_dir = data_dir.dir("properties");
    let vmo = read_only(system_props).get_backing_memory(fio::VmoFlags::READ).await?;
    let _ = properties_dir.add_vmo_file_at("system_properties", vmo);

    let _ = fs.take_and_serve_directory_handle()?;

    let mut application_task_group = fasync::TaskGroup::new();

    while let Some(request) = fs.next().await {
        match configs.get(&request.uuid) {
            Some(config) => application_task_group
                .spawn(fasync::Task::spawn(run_application(request, config.clone()))),
            None => {
                tracing::warn!("Received connection request for unknown UUID {:?}", request.uuid)
            }
        }
    }

    application_task_group.join().await;

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use std::path::Path;
    use tee_properties::{PropEnumerator, PropSet, PropSetType, PropType};

    // This test validates the base platform-specified properties which are hosted in TA manager.
    #[test]
    fn validate_system_properties() {
        let properties_file_path = Path::new("/pkg/data/properties/system_properties");

        let prop_set =
            PropSet::from_config_file(&properties_file_path, PropSetType::TeeImplementation)
                .expect("prop set loads successfully");
        let mut prop_enum = PropEnumerator::new(prop_set);
        prop_enum.start();

        // Will report ItemNotFound error when it moves past the end of the property set.
        while prop_enum.get_property_name().is_ok() {
            let prop_type: PropType =
                prop_enum.get_property_type().expect("get property type successfully");
            let parse_success = match prop_type {
                PropType::BinaryBlock => prop_enum.get_property_as_binary_block().is_ok(),
                PropType::UnsignedInt32 => prop_enum.get_property_as_u32().is_ok(),
                PropType::UnsignedInt64 => prop_enum.get_property_as_u64().is_ok(),
                PropType::Boolean => prop_enum.get_property_as_bool().is_ok(),
                PropType::Uuid => prop_enum.get_property_as_uuid().is_ok(),
                PropType::Identity => prop_enum.get_property_as_identity().is_ok(),
                PropType::String => prop_enum.get_property_as_string().is_ok(),
            };
            assert!(
                parse_success,
                "Bad value found, parsing expected to fail: {:?}",
                prop_enum.get_property_name().expect("get name successfully")
            );
            prop_enum.next().expect("advance to next successfully");
        }
    }
}
