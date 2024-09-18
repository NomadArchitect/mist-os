// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec::Vec;
use assert_matches::assert_matches;
use ip_test_macro::ip_test;

use net_types::ethernet::Mac;
use net_types::ip::{AddrSubnet, Ipv4, Ipv6, Mtu};
use net_types::SpecifiedAddr;
use netstack3_base::testutil::{TestAddrs, TestIpExt};
use netstack3_core::error::NotFoundError;
use netstack3_core::testutil::{
    CtxPairExt as _, FakeBindingsCtx, FakeCtx, DEFAULT_INTERFACE_METRIC,
};
use netstack3_core::IpExt;
use netstack3_device::loopback::{self, LoopbackCreationProperties, LoopbackDevice};
use netstack3_device::queue::ReceiveQueueContext;
use netstack3_ip::device::IpAddressId as _;
use netstack3_ip::{self as ip};
use packet::{Buf, ParseBuffer as _};
use packet_formats::ethernet::{EthernetFrame, EthernetFrameLengthCheck};

const MTU: Mtu = Mtu::new(66);

#[test]
fn loopback_mtu() {
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<LoopbackDevice>()
        .add_device_with_default_state(
            LoopbackCreationProperties { mtu: MTU },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);

    assert_eq!(ip::IpDeviceMtuContext::<Ipv4>::get_mtu(&mut ctx.core_ctx(), &device), MTU);
    assert_eq!(ip::IpDeviceMtuContext::<Ipv6>::get_mtu(&mut ctx.core_ctx(), &device), MTU);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_loopback_add_remove_addrs<I: TestIpExt + IpExt>() {
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<LoopbackDevice>()
        .add_device_with_default_state(
            LoopbackCreationProperties { mtu: MTU },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);

    let get_addrs = |ctx: &mut FakeCtx| {
        ip::device::IpDeviceStateContext::<I, _>::with_address_ids(
            &mut ctx.core_ctx(),
            &device,
            |addrs, _core_ctx| addrs.map(|a| SpecifiedAddr::from(a.addr())).collect::<Vec<_>>(),
        )
    };

    let TestAddrs { subnet, local_ip, local_mac: _, remote_ip: _, remote_mac: _ } = I::TEST_ADDRS;
    let addr_sub =
        AddrSubnet::from_witness(local_ip, subnet.prefix()).expect("error creating AddrSubnet");

    assert_eq!(get_addrs(&mut ctx), []);

    assert_eq!(ctx.core_api().device_ip::<I>().add_ip_addr_subnet(&device, addr_sub), Ok(()));
    let addr = addr_sub.addr();
    assert_eq!(&get_addrs(&mut ctx)[..], [addr]);

    assert_eq!(
        ctx.core_api().device_ip::<I>().del_ip_addr(&device, addr).unwrap().into_removed(),
        addr_sub
    );
    assert_eq!(get_addrs(&mut ctx), []);

    assert_matches!(ctx.core_api().device_ip::<I>().del_ip_addr(&device, addr), Err(NotFoundError));
}

#[ip_test(I)]
fn loopback_sends_ethernet<I: TestIpExt + IpExt>() {
    let mut ctx = FakeCtx::default();
    let device = ctx.core_api().device::<LoopbackDevice>().add_device_with_default_state(
        LoopbackCreationProperties { mtu: MTU },
        DEFAULT_INTERFACE_METRIC,
    );
    ctx.test_api().enable_device(&device.clone().into());
    let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;

    let destination = ip::IpPacketDestination::<I, _>::from_addr(I::TEST_ADDRS.local_ip);
    const BODY: &[u8] = b"IP body".as_slice();

    let body = Buf::new(Vec::from(BODY), ..);
    loopback::send_ip_frame(&mut core_ctx.context(), bindings_ctx, &device, destination, body)
        .expect("can send");

    // There is no transmit queue so the frames will immediately go into the
    // receive queue.
    let mut frames = ReceiveQueueContext::<LoopbackDevice, _>::with_receive_queue_mut(
        &mut core_ctx.context(),
        &device,
        |queue_state| queue_state.take_frames().map(|(_meta, frame)| frame).collect::<Vec<_>>(),
    );

    let frame = assert_matches!(frames.as_mut_slice(), [frame] => frame);

    let eth = frame
        .parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::NoCheck)
        .expect("is ethernet");
    assert_eq!(eth.src_mac(), Mac::UNSPECIFIED);
    assert_eq!(eth.dst_mac(), Mac::UNSPECIFIED);
    assert_eq!(eth.ethertype(), Some(I::ETHER_TYPE));

    // Trim the body to account for ethernet padding.
    assert_eq!(&frame.as_ref()[..BODY.len()], BODY);

    // Clear all device references.
    ctx.bindings_ctx.state_mut().rx_available.clear();
    ctx.core_api().device().remove_device(device).into_removed();
}
