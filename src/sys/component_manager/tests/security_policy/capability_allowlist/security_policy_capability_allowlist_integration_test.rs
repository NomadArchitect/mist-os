// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_component::client;
use security_policy_test_util::{open_exposed_dir, start_policy_test};
use {fidl_test_policy as ftest, fuchsia_async as fasync};

const COMPONENT_MANAGER_URL: &str = "#meta/cm_for_test.cm";
const ROOT_URL: &str = "#meta/test_root.cm";

#[fasync::run_singlethreaded(test)]
async fn verify_restricted_capability_allowed() -> Result<(), Error> {
    let (_test, realm, _event_stream) = start_policy_test(COMPONENT_MANAGER_URL, ROOT_URL).await?;
    let child_name = "policy_allowed";
    let exposed_dir = open_exposed_dir(&realm, child_name).await.expect("bind should succeed");
    let access_controller =
        client::connect_to_protocol_at_dir_root::<ftest::AccessMarker>(&exposed_dir)?;
    assert!(access_controller.access_restricted_protocol().await?);
    assert!(access_controller.access_restricted_directory().await?);
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn verify_restrited_capability_disallowed() -> Result<(), Error> {
    let (_test, realm, _event_stream) = start_policy_test(COMPONENT_MANAGER_URL, ROOT_URL).await?;
    let child_name = "policy_denied";
    let exposed_dir = open_exposed_dir(&realm, child_name).await.expect("bind should succeed");
    let access_controller =
        client::connect_to_protocol_at_dir_root::<ftest::AccessMarker>(&exposed_dir)?;
    assert_eq!(access_controller.access_restricted_protocol().await?, false);
    assert_eq!(access_controller.access_restricted_directory().await?, false);
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn verify_unrestricted_capability_allowed() -> Result<(), Error> {
    let (_test, realm, _event_stream) = start_policy_test(COMPONENT_MANAGER_URL, ROOT_URL).await?;
    let child_name = "policy_not_violated";
    let exposed_dir = open_exposed_dir(&realm, child_name).await.expect("bind should succeed");
    let access_controller =
        client::connect_to_protocol_at_dir_root::<ftest::AccessMarker>(&exposed_dir)?;
    assert!(access_controller.access_unrestricted_protocol().await?);
    assert!(access_controller.access_unrestricted_directory().await?);
    Ok(())
}
