// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use assert_matches::assert_matches;
use component_events::events::*;
use component_events::matcher::*;
use component_events::sequence::{EventSequence, Ordering};
use fuchsia_component::client;
use security_policy_test_util::{open_exposed_dir, start_policy_test};
use zx::{self as zx, AsHandleRef};
use {fidl_fuchsia_component as fcomponent, fidl_test_policy as ftest, fuchsia_async as fasync};

const CM_URL: &str = "#meta/cm_for_test.cm";
const ROOT_URL: &str = "#meta/test_root.cm";

#[fasync::run_singlethreaded(test)]
async fn verify_ambient_vmex_default_denied() -> Result<(), Error> {
    let (_test, realm, _event_stream) = start_policy_test(CM_URL, ROOT_URL).await?;

    let child_name = "policy_not_requested";
    let exposed_dir = open_exposed_dir(&realm, child_name).await.expect("bind should succeed");
    let ops =
        client::connect_to_protocol_at_dir_root::<ftest::ProtectedOperationsMarker>(&exposed_dir)
            .context("failed to connect to test service after bind")?;

    let vmo = zx::Vmo::create(1).unwrap();
    let result = ops.ambient_replace_as_executable(vmo).await.context("fidl call failed")?;
    assert_matches!(result.map_err(zx::Status::from_raw), Err(zx::Status::ACCESS_DENIED));

    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn verify_ambient_vmex_allowed() -> Result<(), Error> {
    let (_test, realm, _event_stream) = start_policy_test(CM_URL, ROOT_URL).await?;
    let child_name = "policy_allowed";
    let exposed_dir = open_exposed_dir(&realm, child_name).await.expect("bind should succeed");
    let ops =
        client::connect_to_protocol_at_dir_root::<ftest::ProtectedOperationsMarker>(&exposed_dir)
            .context("failed to connect to test service after bind")?;

    let vmo = zx::Vmo::create(1).unwrap();
    let result = ops.ambient_replace_as_executable(vmo).await.context("fidl call failed")?;
    match result.map_err(zx::Status::from_raw) {
        Ok(exec_vmo) => {
            assert!(exec_vmo.basic_info().unwrap().rights.contains(zx::Rights::EXECUTE));
        }
        Err(zx::Status::ACCESS_DENIED) => {
            panic!("Unexpected ACCESS_DENIED when policy should be allowed")
        }
        Err(err) => panic!("Unexpected error {}", err),
    }

    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn verify_ambient_vmex_denied() -> Result<(), Error> {
    let (_test, realm, event_stream) = start_policy_test(CM_URL, ROOT_URL).await?;

    // This security policy is enforced inside the ELF runner. The component will fail to launch
    // because of the denial, but the connection to fuchsia.component/Binder
    // will be successful because it's async. We watch for the Started & Stopped
    // event to detect launch failure.
    let child_name = "policy_denied";
    let exposed_dir =
        open_exposed_dir(&realm, child_name).await.expect("open_exposed_dir should succeed");
    client::connect_to_protocol_at_dir_root::<fcomponent::BinderMarker>(&exposed_dir)
        .context("failed to connect to fuchsia.component.Binder of child")?;
    let moniker = format!("./root/{}", child_name);
    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok().r#type(Started::TYPE).moniker(moniker.clone()),
                EventMatcher::ok().r#type(Stopped::TYPE).moniker(moniker.clone()),
            ],
            Ordering::Unordered,
        )
        .expect(event_stream)
        .await
        .unwrap();

    Ok(())
}
