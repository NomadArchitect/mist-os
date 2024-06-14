// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This actor has one action that fails immediately.

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use futures::FutureExt;
use rand::rngs::SmallRng;
use stress_test_actor::{actor_loop, Action};

#[fuchsia::main(logging = false)]
pub async fn main() -> Result<()> {
    // TODO(84952): This syntax is complex and can be simplified using Rust macros.
    actor_loop((), vec![Action { name: "fails_immediately", run: fails_immediately }]).await
}

pub fn fails_immediately<'a>(_: &'a mut (), _: SmallRng) -> BoxFuture<'a, Result<()>> {
    async move { Err(anyhow!("This action is expected to fail immediately")) }.boxed()
}
