// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares the API for configuring multicast forwarding within the netstack.

use net_types::ip::{Ip, IpVersionMarker};
use netstack3_base::{AnyDevice, ContextPair, DeviceIdContext};

use crate::internal::multicast_forwarding::route::{Action, MulticastRoute, MulticastRouteKey};
use crate::internal::multicast_forwarding::state::{
    MulticastForwardingPendingPacketsContext as _, MulticastForwardingState,
    MulticastForwardingStateContext, MulticastRouteTableContext as _,
};
use crate::IpLayerIpExt;

/// The API action can not be performed while multicast forwarding is disabled.
#[derive(Debug, Eq, PartialEq)]
pub struct MulticastForwardingDisabledError {}

/// The multicast forwarding API.
pub struct MulticastForwardingApi<I: Ip, C> {
    ctx: C,
    _ip_mark: IpVersionMarker<I>,
}

impl<I: Ip, C> MulticastForwardingApi<I, C> {
    /// Constructs a new multicast forwarding API.
    pub fn new(ctx: C) -> Self {
        Self { ctx, _ip_mark: IpVersionMarker::new() }
    }
}

impl<I: IpLayerIpExt, C> MulticastForwardingApi<I, C>
where
    C: ContextPair,
    C::CoreContext: MulticastForwardingStateContext<I>,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self { ctx, _ip_mark } = self;
        ctx.core_ctx()
    }

    /// Enables multicast forwarding.
    ///
    /// Returns whether multicast forwarding was newly enabled.
    pub fn enable(&mut self) -> bool {
        self.core_ctx().with_state_mut(|state, _ctx| match state {
            MulticastForwardingState::Enabled(_) => false,
            MulticastForwardingState::Disabled => {
                *state = MulticastForwardingState::Enabled(Default::default());
                true
            }
        })
    }

    /// Disables multicast forwarding.
    ///
    /// Returns whether multicast forwarding was newly disabled.
    ///
    /// Upon being disabled, the multicast route table will be cleared,
    /// and all pending packets will be dropped.
    pub fn disable(&mut self) -> bool {
        self.core_ctx().with_state_mut(|state, _ctx| match state {
            MulticastForwardingState::Disabled => false,
            MulticastForwardingState::Enabled(_) => {
                *state = MulticastForwardingState::Disabled;
                true
            }
        })
    }

    /// Add the route to the multicast route table.
    ///
    /// If a route already exists with the same key, it will be replaced, and
    /// the original route will be returned.
    pub fn add_multicast_route(
        &mut self,
        key: MulticastRouteKey<I>,
        route: MulticastRoute<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    ) -> Result<
        Option<MulticastRoute<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>>,
        MulticastForwardingDisabledError,
    > {
        self.core_ctx().with_state_mut(|state, ctx| {
            let state = match state {
                MulticastForwardingState::Disabled => {
                    return Err(MulticastForwardingDisabledError {})
                }
                MulticastForwardingState::Enabled(state) => state,
            };
            ctx.with_route_table_mut(state, |route_table, ctx| {
                let orig_route = route_table.insert(key, route);
                // NB: Only try to send pending packets if the route was newly
                // installed. Any existing route would not have pending packets,
                // as per the key-invariant on the route table.
                match &orig_route {
                    Some(_route) => {
                        #[cfg(debug_assertions)]
                        ctx.with_pending_table_mut(state, |_pending_table| {
                            // TODO(https://fxbug.dev/353328975): Debug assert
                            // that `key` is absent in the pending table.
                        })
                    }
                    None => {
                        ctx.with_pending_table_mut(state, |_pending_table| {
                            // TODO(https://fxbug.dev/353328975): Send any
                            // pending packets that were waiting for this route
                            // to be installed.
                        });
                    }
                }
                Ok(orig_route)
            })
        })
    }

    /// Remove the route from the multicast route table.
    ///
    /// Returns `None` if the route did not exist.
    pub fn remove_multicast_route(
        &mut self,
        key: &MulticastRouteKey<I>,
    ) -> Result<
        Option<MulticastRoute<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>>,
        MulticastForwardingDisabledError,
    > {
        self.core_ctx().with_state_mut(|state, ctx| {
            let state = match state {
                MulticastForwardingState::Disabled => {
                    return Err(MulticastForwardingDisabledError {})
                }
                MulticastForwardingState::Enabled(state) => state,
            };
            ctx.with_route_table_mut(state, |route_table, _ctx| Ok(route_table.remove(key)))
        })
    }

    /// Remove all references to the device from the multicast forwarding state.
    ///
    /// Typically, this is called as part of device removal to purge all strong
    /// device references.
    ///
    /// Any routes that reference the device as an `input_interface` will be
    /// removed. Any routes that reference the device as a
    /// [`MulticastRouteTarget`] will have that target removed (and will
    /// themselves be removed if it's the only target).
    pub fn remove_references_to_device(
        &mut self,
        dev: &<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
    ) {
        self.core_ctx().with_state_mut(|state, ctx| {
            let state = match state {
                // There's no state to update if forwarding is disabled.
                MulticastForwardingState::Disabled => return,
                MulticastForwardingState::Enabled(state) => state,
            };
            ctx.with_route_table_mut(state, |route_table, _ctx| {
                route_table.retain(|_route_key, MulticastRoute { action, input_interface }| {
                    if dev == &*input_interface {
                        return false;
                    }
                    match action {
                        Action::Forward(ref mut targets) => {
                            targets.retain(|target| dev != &target.output_interface);
                            if targets.is_empty() {
                                return false;
                            }
                        }
                    }
                    true
                })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use core::cell::RefCell;
    use core::ops::Deref;

    use alloc::rc::Rc;
    use alloc::vec;
    use assert_matches::assert_matches;
    use derivative::Derivative;
    use ip_test_macro::ip_test;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::ip::{Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
    use netstack3_base::testutil::{FakeStrongDeviceId, MultipleDevicesId};
    use netstack3_base::{CtxPair, StrongDeviceIdentifier};

    use crate::multicast_forwarding::{
        MulticastForwardingEnabledState, MulticastForwardingPendingPackets,
        MulticastForwardingPendingPacketsContext, MulticastRoute, MulticastRouteKey,
        MulticastRouteTable, MulticastRouteTableContext, MulticastRouteTarget,
    };

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    struct FakeCoreCtxState<I: IpLayerIpExt, D: FakeStrongDeviceId> {
        // NB: Hold in an `Rc<RefCell<...>>` to switch to runtime borrow
        // checking. This allows us to borrow the multicast forwarding state at
        // the same time as the outer `FakeCoreCtx` is mutably borrowed.
        multicast_forwarding: Rc<RefCell<MulticastForwardingState<I, D>>>,
    }

    type FakeBindingsCtx = netstack3_base::testutil::FakeBindingsCtx<(), (), (), ()>;
    type FakeCoreCtx<I, D> = netstack3_base::testutil::FakeCoreCtx<FakeCoreCtxState<I, D>, (), D>;

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> MulticastForwardingStateContext<I>
        for FakeCoreCtx<I, D>
    {
        type Ctx<'a> = FakeCoreCtx<I, D>;
        fn with_state<
            O,
            F: FnOnce(&MulticastForwardingState<I, Self::DeviceId>, &mut Self::Ctx<'_>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let state = self.state.multicast_forwarding.clone();
            let borrow = state.borrow();
            cb(&borrow, self)
        }
        fn with_state_mut<
            O,
            F: FnOnce(&mut MulticastForwardingState<I, Self::DeviceId>, &mut Self::Ctx<'_>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let state = self.state.multicast_forwarding.clone();
            let mut borrow = state.borrow_mut();
            cb(&mut borrow, self)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> MulticastRouteTableContext<I> for FakeCoreCtx<I, D> {
        type Ctx<'a> = FakeCoreCtx<I, D>;
        fn with_route_table<
            O,
            F: FnOnce(&MulticastRouteTable<I, Self::DeviceId>, &mut Self::Ctx<'_>) -> O,
        >(
            &mut self,
            state: &MulticastForwardingEnabledState<I, Self::DeviceId>,
            cb: F,
        ) -> O {
            let route_table = state.route_table().read();
            cb(&route_table, self)
        }
        fn with_route_table_mut<
            O,
            F: FnOnce(&mut MulticastRouteTable<I, Self::DeviceId>, &mut Self::Ctx<'_>) -> O,
        >(
            &mut self,
            state: &MulticastForwardingEnabledState<I, Self::DeviceId>,
            cb: F,
        ) -> O {
            let mut route_table = state.route_table().write();
            cb(&mut route_table, self)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> MulticastForwardingPendingPacketsContext<I>
        for FakeCoreCtx<I, D>
    {
        fn with_pending_table_mut<
            O,
            F: FnOnce(&mut MulticastForwardingPendingPackets<I, Self::DeviceId>) -> O,
        >(
            &mut self,
            state: &MulticastForwardingEnabledState<I, Self::DeviceId>,
            cb: F,
        ) -> O {
            let mut pending_table = state.pending_table().lock();
            cb(&mut pending_table)
        }
    }

    fn new_multicast_forwarding_api<I: IpLayerIpExt>(
    ) -> MulticastForwardingApi<I, CtxPair<FakeCoreCtx<I, MultipleDevicesId>, FakeBindingsCtx>>
    {
        MulticastForwardingApi::new(CtxPair::with_core_ctx(FakeCoreCtx::with_state(
            Default::default(),
        )))
    }

    /// An IP extension trait providing constants for various IP addresses.
    trait ConstantsIpExt: Ip {
        const SRC1: Self::Addr;
        const SRC2: Self::Addr;
        const DST1: Self::Addr;
        const DST2: Self::Addr;
    }

    impl ConstantsIpExt for Ipv4 {
        const SRC1: Ipv4Addr = net_ip_v4!("192.0.2.1");
        const SRC2: Ipv4Addr = net_ip_v4!("192.0.2.2");
        const DST1: Ipv4Addr = net_ip_v4!("224.0.0.1");
        const DST2: Ipv4Addr = net_ip_v4!("224.0.0.2");
    }

    impl ConstantsIpExt for Ipv6 {
        const SRC1: Ipv6Addr = net_ip_v6!("2001:0DB8::1");
        const SRC2: Ipv6Addr = net_ip_v6!("2001:0DB8::2");
        const DST1: Ipv6Addr = net_ip_v6!("ff0e::1");
        const DST2: Ipv6Addr = net_ip_v6!("ff0e::2");
    }

    #[ip_test(I)]
    fn enable_disable<I: IpLayerIpExt>() {
        let mut api = new_multicast_forwarding_api::<I>();

        assert_matches!(
            api.core_ctx().state.multicast_forwarding.borrow().deref(),
            &MulticastForwardingState::Disabled
        );
        assert!(api.enable());
        assert!(!api.enable());
        assert_matches!(
            api.core_ctx().state.multicast_forwarding.borrow().deref(),
            &MulticastForwardingState::Enabled(_)
        );
        assert!(api.disable());
        assert!(!api.disable());
        assert_matches!(
            api.core_ctx().state.multicast_forwarding.borrow().deref(),
            &MulticastForwardingState::Disabled
        );
    }

    #[ip_test(I)]
    fn add_remove_route<I: IpLayerIpExt + ConstantsIpExt>() {
        let key1 = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let key2 = MulticastRouteKey::new(I::SRC2, I::DST2).unwrap();
        let forward_to_b = MulticastRoute::new_forward(
            MultipleDevicesId::A,
            vec![MulticastRouteTarget { output_interface: MultipleDevicesId::B, min_ttl: 0 }],
        )
        .unwrap();
        let forward_to_c = MulticastRoute::new_forward(
            MultipleDevicesId::A,
            vec![MulticastRouteTarget { output_interface: MultipleDevicesId::C, min_ttl: 0 }],
        )
        .unwrap();

        let mut api = new_multicast_forwarding_api::<I>();

        // Adding/removing routes before multicast forwarding is enabled should
        // fail.
        assert_eq!(
            api.add_multicast_route(key1.clone(), forward_to_b.clone()),
            Err(MulticastForwardingDisabledError {})
        );
        assert_eq!(api.remove_multicast_route(&key1), Err(MulticastForwardingDisabledError {}));

        // Enable the API and observe success.
        assert!(api.enable());
        assert_eq!(api.add_multicast_route(key1.clone(), forward_to_b.clone()), Ok(None));
        assert_eq!(api.remove_multicast_route(&key1), Ok(Some(forward_to_b.clone())));

        // Removing a route that doesn't exist should return `None`.
        assert_eq!(api.remove_multicast_route(&key1), Ok(None));

        // Adding a route with the same key as an existing route should
        // overwrite the original.
        assert_eq!(api.add_multicast_route(key1.clone(), forward_to_b.clone()), Ok(None));
        assert_eq!(
            api.add_multicast_route(key1.clone(), forward_to_c.clone()),
            Ok(Some(forward_to_b.clone()))
        );
        assert_eq!(api.remove_multicast_route(&key1), Ok(Some(forward_to_c.clone())));

        // Routes with different keys can co-exist.
        assert_eq!(api.add_multicast_route(key1.clone(), forward_to_b.clone()), Ok(None));
        assert_eq!(api.add_multicast_route(key2.clone(), forward_to_c.clone()), Ok(None));
        assert_eq!(api.remove_multicast_route(&key1), Ok(Some(forward_to_b)));
        assert_eq!(api.remove_multicast_route(&key2), Ok(Some(forward_to_c)));
    }

    #[ip_test(I)]
    fn remove_references_to_device<I: IpLayerIpExt + ConstantsIpExt>() {
        // NB: 4 arbitrary keys, that are unique from each other.
        let key1 = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let key2 = MulticastRouteKey::new(I::SRC2, I::DST1).unwrap();
        let key3 = MulticastRouteKey::new(I::SRC1, I::DST2).unwrap();
        let key4 = MulticastRouteKey::new(I::SRC2, I::DST2).unwrap();

        // Create 4 routes, each exercising a different edge case.
        const GOOD_DEV1: MultipleDevicesId = MultipleDevicesId::A;
        const GOOD_DEV2: MultipleDevicesId = MultipleDevicesId::B;
        const BAD_DEV: MultipleDevicesId = MultipleDevicesId::C;
        const GOOD_TARGET1: MulticastRouteTarget<MultipleDevicesId> =
            MulticastRouteTarget { output_interface: GOOD_DEV1, min_ttl: 0 };
        const GOOD_TARGET2: MulticastRouteTarget<MultipleDevicesId> =
            MulticastRouteTarget { output_interface: GOOD_DEV2, min_ttl: 0 };
        const BAD_TARGET: MulticastRouteTarget<MultipleDevicesId> =
            MulticastRouteTarget { output_interface: BAD_DEV, min_ttl: 0 };
        let dev_is_input = MulticastRoute::new_forward(BAD_DEV, vec![GOOD_TARGET1]).unwrap();
        let dev_is_only_output = MulticastRoute::new_forward(GOOD_DEV1, vec![BAD_TARGET]).unwrap();
        let dev_is_one_output =
            MulticastRoute::new_forward(GOOD_DEV1, vec![GOOD_TARGET2, BAD_TARGET]).unwrap();
        let no_ref_to_dev = MulticastRoute::new_forward(GOOD_DEV1, vec![GOOD_TARGET2]).unwrap();

        // Verify that removing device references is a no-op when multicast
        // forwarding is disabled.
        let mut api = new_multicast_forwarding_api::<I>();
        api.remove_references_to_device(&BAD_DEV.downgrade());
        assert!(api.enable());

        // Add the four routes, remove references to `Dev`, and verify that:
        // * `dev_is_input` & `dev_is_only_output`, were both removed.
        // * `dev_is_one_output` was updated to not list the dev in its
        //    targets.
        // * `no_ref_to_dev` was not updated.
        assert_eq!(api.add_multicast_route(key1.clone(), dev_is_input), Ok(None));
        assert_eq!(api.add_multicast_route(key2.clone(), dev_is_only_output), Ok(None));
        assert_eq!(api.add_multicast_route(key3.clone(), dev_is_one_output), Ok(None));
        assert_eq!(api.add_multicast_route(key4.clone(), no_ref_to_dev.clone()), Ok(None));
        api.remove_references_to_device(&BAD_DEV.downgrade());
        assert_eq!(api.remove_multicast_route(&key1), Ok(None));
        assert_eq!(api.remove_multicast_route(&key2), Ok(None));
        // NB: Equal to `dev_is_one_output`, but with `BAD_TARGET` removed.
        assert_eq!(
            api.remove_multicast_route(&key3),
            Ok(Some(MulticastRoute::new_forward(GOOD_DEV1, vec![GOOD_TARGET2]).unwrap()))
        );
        assert_eq!(api.remove_multicast_route(&key4), Ok(Some(no_ref_to_dev)));
    }
}
