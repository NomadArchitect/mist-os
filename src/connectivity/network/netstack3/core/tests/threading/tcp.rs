// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::{NonZeroU16, NonZeroUsize};

use const_unwrap::const_unwrap_option;
use ip_test_macro::ip_test;
use loom::sync::Arc;
use net_types::ZonedAddr;
use netstack3_core::device::LoopbackDevice;
use netstack3_core::testutil::{CtxPairExt as _, FakeBindingsCtx, FakeCtx};
use netstack3_core::types::WorkQueueReport;
use netstack3_core::{CtxPair, IpExt};
use netstack3_tcp::testutil::{ProvidedBuffers, WriteBackClientBuffers};
use test_case::test_case;

use super::{loom_model, loom_spawn, low_preemption_bound_model};

#[derive(Debug, Copy, Clone)]
enum ServerOrClient {
    Server,
    Client,
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(ServerOrClient::Server; "server")]
#[test_case(ServerOrClient::Client; "client")]
fn race_connect_close<I: IpExt>(close_which: ServerOrClient) {
    loom_model(low_preemption_bound_model(), move || {
        const SERVER_PORT: NonZeroU16 = const_unwrap_option(NonZeroU16::new(22222));
        const BACKLOG: NonZeroUsize = const_unwrap_option(NonZeroUsize::new(1));
        let FakeCtx { core_ctx, bindings_ctx } = FakeCtx::default();
        let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };
        let lo = ctx.test_api().add_loopback();
        let mut tcp_api = ctx.core_api().tcp::<I>();
        let server = tcp_api.create(ProvidedBuffers::Buffers(WriteBackClientBuffers::default()));

        tcp_api.bind(&server, None, Some(SERVER_PORT)).unwrap();
        tcp_api.listen(&server, BACKLOG).unwrap();
        let client = tcp_api.create(ProvidedBuffers::Buffers(WriteBackClientBuffers::default()));
        tcp_api
            .connect(&client, ZonedAddr::Unzoned(I::LOOPBACK_ADDRESS).into(), SERVER_PORT)
            .unwrap();

        let (close_socket, keep_socket) = match close_which {
            ServerOrClient::Server => (server, client),
            ServerOrClient::Client => (client, server),
        };

        // The client's initial SYN is sitting in the loopback rx queue.
        //
        // Race two operations:
        //
        // 1. Closing the one of the sockets.
        // 2. Operating the loopback queue, which will advance the server
        //    state-machine and potentially send a SYN-ACK back.

        let thread_vars = (ctx.clone(), close_socket);
        let t_close = loom_spawn(move || {
            let (mut ctx, close_socket) = thread_vars;
            ctx.core_api().tcp::<I>().close(close_socket);
        });
        let thread_vars = (ctx.clone(), lo.clone());
        let t_recv = loom_spawn(move || {
            let (mut ctx, lo) = thread_vars;

            // Run the loopback queue for as long as we observe rx available
            // signals in the bindings context.
            while !core::mem::take(&mut ctx.bindings_ctx.state_mut().rx_available).is_empty() {
                assert_eq!(
                    ctx.core_api().receive_queue::<LoopbackDevice>().handle_queued_frames(&lo),
                    WorkQueueReport::AllDone
                );
            }
        });

        t_close.join().unwrap();
        t_recv.join().unwrap();

        // Clean up all resources.
        ctx.core_api().tcp::<I>().close(keep_socket);
        ctx.bindings_ctx.state_mut().rx_available.clear();
        ctx.test_api().clear_routes_and_remove_device(lo);
    })
}
