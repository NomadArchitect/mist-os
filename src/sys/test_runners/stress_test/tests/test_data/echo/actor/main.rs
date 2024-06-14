// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This actor has one action that sends an Echo to the subject.

use anyhow::{anyhow, bail, Result};
use fidl_fidl_examples_routing_echo::{EchoMarker, EchoProxy};
use fuchsia_component::client::connect_to_protocol;
use futures::future::BoxFuture;
use futures::FutureExt;
use rand::rngs::SmallRng;
use stress_test_actor::{actor_loop, Action};

const ECHO_TEXT: &'static str = "This is a test";

#[fuchsia::main(logging = false)]
pub async fn main() -> Result<()> {
    // Connect to the Echo protocol
    let echo = connect_to_protocol::<EchoMarker>()?;

    // TODO(84952): This syntax is complex and can be simplified using Rust macros.
    actor_loop(echo, vec![Action { name: "echo_string", run: echo_string }]).await
}

pub fn echo_string<'a>(echo: &'a mut EchoProxy, _: SmallRng) -> BoxFuture<'a, Result<()>> {
    async move {
        let response =
            echo.echo_string(Some(ECHO_TEXT)).await?.ok_or(anyhow!("No string in response"))?;
        if response != ECHO_TEXT {
            bail!("Unexpected response from echo subject");
        }
        Ok(())
    }
    .boxed()
}
