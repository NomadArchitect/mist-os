// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_test_pwrbtn as test_pwrbtn;
use fuchsia_component::client as fclient;
use tracing::info;

#[fuchsia::test(logging_tags = ["critical-services-integration-test"])]
async fn run() -> Result<(), Error> {
    info!("started");

    let tests_proxy = fclient::connect_to_protocol::<test_pwrbtn::TestsMarker>()?;
    // Run the tests. If this function returns then we know the tests have passed.
    tests_proxy.run().await?;

    info!("test success");
    Ok(())
}
