// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_update_installer_ext::{PrepareFailureReason, State};
use pretty_assertions::assert_eq;

/// When epoch.json is in an unexpected format, we should expect to fail with the Internal reason.
#[fasync::run_singlethreaded(test)]
async fn invalid_epoch() {
    let env = TestEnv::builder().build().await;
    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file(
            "epoch.json",
            json!({
              "version": "1",
              // -1 is not a valid u64.
              "epoch": -1,
            })
            .to_string(),
        );

    let mut attempt = env.start_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailPrepare(PrepareFailureReason::Internal)
    );
}

// When target epoch < current epoch, we should fail with the UnsupportedDowngrade reason.
#[fasync::run_singlethreaded(test)]
async fn unsupported_downgrade() {
    let env = TestEnv::builder().build().await;
    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_epoch_json(SOURCE_EPOCH - 1));

    let mut attempt = env.start_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailPrepare(PrepareFailureReason::UnsupportedDowngrade)
    );
}
