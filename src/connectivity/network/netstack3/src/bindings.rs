// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 bindings.
//!
//! This module provides Fuchsia bindings for the [`netstack3_core`] crate.

#![deny(clippy::redundant_clone)]

#[cfg(test)]
mod integration_tests;

mod debug_fidl_worker;
mod devices;
mod filter;
mod inspect;
mod interfaces_admin;
mod interfaces_watcher;
mod multicast_admin;
mod name_worker;
mod neighbor_worker;
mod netdevice_worker;
mod resource_removal;
mod root_fidl_worker;
mod routes;
mod socket;
mod stack_fidl_worker;

mod timers;
mod util;
mod verifier_worker;

use std::collections::HashMap;
use std::convert::{Infallible as Never, TryFrom as _};
use std::ffi::CStr;
use std::fmt::Debug;
use std::future::Future;
use std::num::TryFromIntError;
use std::ops::Deref;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use assert_matches::assert_matches;
use fidl::endpoints::{DiscoverableProtocolMarker, ProtocolMarker as _, RequestStream};
use fuchsia_inspect::health::Reporter as _;
use futures::channel::mpsc;
use futures::{select, FutureExt as _, StreamExt as _};
use log::{debug, error, info, warn};
use packet::{Buf, BufferMut};
use rand::rngs::OsRng;
use rand::{CryptoRng, RngCore};
use util::{ConversionContext, IntoFidl as _};
use {
    fidl_fuchsia_hardware_network as fhardware_network,
    fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin,
    fidl_fuchsia_net_multicast_admin as fnet_multicast_admin,
    fidl_fuchsia_net_routes_admin as fnet_routes_admin, fuchsia_async as fasync,
    fuchsia_zircon as zx,
};

use devices::{
    BindingId, DeviceIdAndName, DeviceSpecificInfo, Devices, DynamicCommonInfo,
    DynamicEthernetInfo, DynamicNetdeviceInfo, EthernetInfo, LoopbackInfo, PureIpDeviceInfo,
    StaticCommonInfo, StaticNetdeviceInfo, TxTaskState,
};
use interfaces_watcher::{InterfaceEventProducer, InterfaceProperties, InterfaceUpdate};
use resource_removal::{ResourceRemovalSink, ResourceRemovalWorker};

use crate::bindings::interfaces_watcher::AddressPropertiesUpdate;
use crate::bindings::util::TaskWaitGroup;
use net_types::ethernet::Mac;
use net_types::ip::{
    AddrSubnet, AddrSubnetEither, Ip, IpAddr, IpAddress, IpVersion, Ipv4, Ipv6, Mtu,
};
use net_types::SpecifiedAddr;
use netstack3_core::device::{
    DeviceId, DeviceLayerEventDispatcher, DeviceLayerStateTypes, DeviceSendFrameError,
    EthernetDeviceId, LoopbackCreationProperties, LoopbackDevice, LoopbackDeviceId, PureIpDeviceId,
    ReceiveQueueBindingsContext, TransmitQueueBindingsContext, WeakDeviceId,
};
use netstack3_core::error::ExistsError;
use netstack3_core::filter::FilterBindingsTypes;
use netstack3_core::icmp::{IcmpEchoBindingsContext, IcmpEchoBindingsTypes, IcmpSocketId};
use netstack3_core::inspect::{InspectableValue, Inspector};
use netstack3_core::ip::{
    AddIpAddrSubnetError, AddressRemovedReason, IpDeviceConfigurationUpdate, IpDeviceEvent,
    Ipv4DeviceConfigurationUpdate, Ipv6DeviceConfiguration, Ipv6DeviceConfigurationUpdate,
    Lifetime, SlaacConfiguration,
};
use netstack3_core::routes::RawMetric;
use netstack3_core::sync::{DynDebugReferences, RwLock as CoreRwLock};
use netstack3_core::udp::{
    UdpBindingsTypes, UdpPacketMeta, UdpReceiveBindingsContext, UdpSocketId,
};
use netstack3_core::{
    neighbor, DeferredResourceRemovalContext, EventContext, InstantBindingsTypes, InstantContext,
    IpExt, ReferenceNotifiers, RngContext, StackState, TimerBindingsTypes, TimerContext, TimerId,
    TracingContext,
};

pub(crate) use inspect::InspectPublisher;

mod ctx {
    use super::*;
    use thiserror::Error;

    /// Provides an implementation of [`BindingsContext`].
    pub(crate) struct BindingsCtx(Arc<BindingsCtxInner>);

    impl Deref for BindingsCtx {
        type Target = BindingsCtxInner;

        fn deref(&self) -> &BindingsCtxInner {
            let Self(this) = self;
            this.deref()
        }
    }

    pub(crate) struct Ctx {
        // `bindings_ctx` is the first member so all strongly-held references are
        // dropped before primary references held in `core_ctx` are dropped. Note
        // that dropping a primary reference while holding onto strong references
        // will cause a panic. See `netstack3_core::sync::PrimaryRc` for more
        // details.
        bindings_ctx: BindingsCtx,
        core_ctx: Arc<StackState<BindingsCtx>>,
    }

    /// Error observed while attempting to destroy the last remaining clone of `Ctx`.
    #[derive(Debug, Error)]
    pub enum DestructionError {
        /// Another reference of `BindingsCtx` still exists.
        #[error("bindings ctx still has {0} references")]
        BindingsCtxStillCloned(usize),
        /// Another reference of `CoreCtx` still exists.
        #[error("core ctx still has {0} references")]
        CoreCtxStillCloned(usize),
    }

    impl Ctx {
        fn new(
            routes_change_sink: routes::ChangeSink,
            resource_removal: ResourceRemovalSink,
        ) -> Self {
            let mut bindings_ctx =
                BindingsCtx(Arc::new(BindingsCtxInner::new(routes_change_sink, resource_removal)));
            let core_ctx = Arc::new(StackState::new(&mut bindings_ctx));
            Self { bindings_ctx, core_ctx }
        }

        pub(crate) fn bindings_ctx(&self) -> &BindingsCtx {
            &self.bindings_ctx
        }

        // Gets a new `RngImpl` as if we have an implementation of `RngContext`,
        // but without needing `&mut self`.
        pub(crate) fn rng(&self) -> RngImpl {
            RngImpl::new()
        }

        /// Destroys the last standing clone of [`Ctx`].
        pub(crate) fn try_destroy_last(self) -> Result<(), DestructionError> {
            let Self { bindings_ctx: BindingsCtx(bindings_ctx), core_ctx } = self;

            fn unwrap_and_drop_or_get_count<T>(arc: Arc<T>) -> Result<(), usize> {
                match Arc::try_unwrap(arc) {
                    Ok(t) => Ok(std::mem::drop(t)),
                    Err(arc) => Err(Arc::strong_count(&arc)),
                }
            }

            // Always destroy bindings ctx first.
            unwrap_and_drop_or_get_count(bindings_ctx)
                .map_err(DestructionError::BindingsCtxStillCloned)?;
            unwrap_and_drop_or_get_count(core_ctx).map_err(DestructionError::CoreCtxStillCloned)
        }

        pub(crate) fn api(&mut self) -> netstack3_core::CoreApi<'_, &mut BindingsCtx> {
            let Ctx { bindings_ctx, core_ctx } = self;
            core_ctx.api(bindings_ctx)
        }
    }

    impl Clone for Ctx {
        fn clone(&self) -> Self {
            let Self { bindings_ctx: BindingsCtx(inner), core_ctx } = self;
            Self { bindings_ctx: BindingsCtx(inner.clone()), core_ctx: core_ctx.clone() }
        }
    }

    /// Contains the information needed to start serving a network stack over FIDL.
    pub(crate) struct NetstackSeed {
        pub(crate) netstack: Netstack,
        pub(crate) interfaces_worker: interfaces_watcher::Worker,
        pub(crate) interfaces_watcher_sink: interfaces_watcher::WorkerWatcherSink,
        pub(crate) routes_change_runner: routes::ChangeRunner,
        pub(crate) neighbor_worker: neighbor_worker::Worker,
        pub(crate) neighbor_watcher_sink: mpsc::Sender<neighbor_worker::NewWatcher>,
        pub(crate) resource_removal_worker: ResourceRemovalWorker,
    }

    impl Default for NetstackSeed {
        fn default() -> Self {
            let (interfaces_worker, interfaces_watcher_sink, interfaces_event_sink) =
                interfaces_watcher::Worker::new();
            let (routes_change_sink, routes_change_runner) = routes::create_sink_and_runner();
            let (resource_removal_worker, resource_removal_sink) = ResourceRemovalWorker::new();
            let ctx = Ctx::new(routes_change_sink, resource_removal_sink);
            let (neighbor_worker, neighbor_watcher_sink, neighbor_event_sink) =
                neighbor_worker::new_worker();
            Self {
                netstack: Netstack { ctx, interfaces_event_sink, neighbor_event_sink },
                interfaces_worker,
                interfaces_watcher_sink,
                routes_change_runner,
                neighbor_worker,
                neighbor_watcher_sink,
                resource_removal_worker,
            }
        }
    }
}

pub(crate) use ctx::{BindingsCtx, Ctx, NetstackSeed};

/// Extends the methods available to [`DeviceId`].
trait DeviceIdExt {
    /// Returns the state associated with devices.
    fn external_state(&self) -> DeviceSpecificInfo<'_>;
}

impl DeviceIdExt for DeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        match self {
            DeviceId::Ethernet(d) => DeviceSpecificInfo::Ethernet(d.external_state()),
            DeviceId::Loopback(d) => DeviceSpecificInfo::Loopback(d.external_state()),
            DeviceId::PureIp(d) => DeviceSpecificInfo::PureIp(d.external_state()),
        }
    }
}

impl DeviceIdExt for EthernetDeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        DeviceSpecificInfo::Ethernet(self.external_state())
    }
}

impl DeviceIdExt for LoopbackDeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        DeviceSpecificInfo::Loopback(self.external_state())
    }
}

impl DeviceIdExt for PureIpDeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        DeviceSpecificInfo::PureIp(self.external_state())
    }
}

/// Extends the methods available to [`Lifetime`].
trait LifetimeExt {
    /// Converts `self` to `zx::MonotonicTime`.
    fn into_zx_time(self) -> zx::MonotonicTime;
}

impl LifetimeExt for Lifetime<StackTime> {
    fn into_zx_time(self) -> zx::MonotonicTime {
        match self {
            Lifetime::Finite(StackTime(time)) => time.into_zx(),
            Lifetime::Infinite => zx::MonotonicTime::INFINITE,
        }
    }
}

const LOOPBACK_NAME: &'static str = "lo";

/// Default MTU for loopback.
///
/// This value is also the default value used on Linux. As of writing:
///
/// ```shell
/// $ ip link show dev lo
/// 1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000
///     link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
/// ```
const DEFAULT_LOOPBACK_MTU: Mtu = Mtu::new(65536);

/// Default routing metric for newly created interfaces, if unspecified.
///
/// The value is currently kept in sync with the Netstack2 implementation.
const DEFAULT_INTERFACE_METRIC: u32 = 100;

pub(crate) struct BindingsCtxInner {
    timers: timers::TimerDispatcher<TimerId<BindingsCtx>>,
    devices: Devices<DeviceId<BindingsCtx>>,
    routes: routes::ChangeSink,
    resource_removal: ResourceRemovalSink,
}

impl BindingsCtxInner {
    fn new(routes_change_sink: routes::ChangeSink, resource_removal: ResourceRemovalSink) -> Self {
        Self {
            timers: Default::default(),
            devices: Default::default(),
            routes: routes_change_sink,
            resource_removal,
        }
    }
}

impl AsRef<Devices<DeviceId<BindingsCtx>>> for BindingsCtx {
    fn as_ref(&self) -> &Devices<DeviceId<BindingsCtx>> {
        &self.devices
    }
}

impl<D> ConversionContext for D
where
    D: AsRef<Devices<DeviceId<BindingsCtx>>>,
{
    fn get_core_id(&self, binding_id: BindingId) -> Option<DeviceId<BindingsCtx>> {
        self.as_ref().get_core_id(binding_id)
    }

    fn get_binding_id(&self, core_id: DeviceId<BindingsCtx>) -> BindingId {
        core_id.bindings_id().id
    }
}

/// A thin wrapper around `fuchsia_async::Time` that implements `core::Instant`.
#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone, Debug)]
pub(crate) struct StackTime(fasync::Time);

impl netstack3_core::Instant for StackTime {
    fn checked_duration_since(&self, earlier: StackTime) -> Option<Duration> {
        match u64::try_from(self.0.into_nanos() - earlier.0.into_nanos()) {
            Ok(nanos) => Some(Duration::from_nanos(nanos)),
            Err(TryFromIntError { .. }) => None,
        }
    }

    fn checked_add(&self, duration: Duration) -> Option<StackTime> {
        Some(StackTime(fasync::Time::from_nanos(
            self.0.into_nanos().checked_add(i64::try_from(duration.as_nanos()).ok()?)?,
        )))
    }

    fn checked_sub(&self, duration: Duration) -> Option<StackTime> {
        Some(StackTime(fasync::Time::from_nanos(
            self.0.into_nanos().checked_sub(i64::try_from(duration.as_nanos()).ok()?)?,
        )))
    }
}

impl std::fmt::Display for StackTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(time) = *self;
        write!(f, "{:.6}", time.into_nanos() as f64 / 1_000_000_000f64)
    }
}

impl InspectableValue for StackTime {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        let Self(inner) = self;
        inspector.record_int(name, inner.into_nanos())
    }
}

impl InstantBindingsTypes for BindingsCtx {
    type Instant = StackTime;
}

impl InstantContext for BindingsCtx {
    fn now(&self) -> StackTime {
        StackTime(fasync::Time::now())
    }
}

impl TracingContext for BindingsCtx {
    type DurationScope = fuchsia_trace::DurationScope<'static>;

    fn duration(&self, name: &'static CStr) -> fuchsia_trace::DurationScope<'static> {
        fuchsia_trace::duration(c"net", name, &[])
    }
}

/// Convenience wrapper around the [`fuchsia_trace::duration`] macro that always
/// uses the "net" tracing category.
///
/// [`fuchsia_trace::duration`] uses RAII to begin and end the duration by tying
/// the scope of the duration to the lifetime of the object it returns. This
/// macro encapsulates that logic such that the trace duration will end when the
/// scope in which the macro is called ends.
macro_rules! trace_duration {
    ($name:expr) => {
        fuchsia_trace::duration!(c"net", $name);
    };
}

pub(crate) use trace_duration;

impl FilterBindingsTypes for BindingsCtx {
    type DeviceClass = fidl_fuchsia_net_interfaces::PortClass;
}

#[derive(Default)]
pub(crate) struct RngImpl;

impl RngImpl {
    fn new() -> Self {
        // A change detector in case OsRng is no longer a ZST and we should keep
        // state for it inside RngImpl.
        let OsRng {} = OsRng::default();
        RngImpl {}
    }
}

/// [`RngCore`] for `RngImpl` relies entirely on the operating system to
/// generate random numbers and it needs not keep any state itself.
///
/// [`OsRng`] is a zero-sized type that provides randomness from the OS.
impl RngCore for RngImpl {
    fn next_u32(&mut self) -> u32 {
        OsRng::default().next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        OsRng::default().next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        OsRng::default().fill_bytes(dest)
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        OsRng::default().try_fill_bytes(dest)
    }
}

impl CryptoRng for RngImpl where OsRng: CryptoRng {}

impl RngContext for BindingsCtx {
    type Rng<'a> = RngImpl;

    fn rng(&mut self) -> RngImpl {
        RngImpl::new()
    }
}

impl TimerBindingsTypes for BindingsCtx {
    type Timer = timers::Timer<TimerId<BindingsCtx>>;
    type DispatchId = TimerId<BindingsCtx>;
    type UniqueTimerId = timers::UniqueTimerId<TimerId<BindingsCtx>>;
}

impl TimerContext for BindingsCtx {
    fn new_timer(&mut self, id: Self::DispatchId) -> Self::Timer {
        self.timers.new_timer(id)
    }

    fn schedule_timer_instant(
        &mut self,
        StackTime(time): Self::Instant,
        timer: &mut Self::Timer,
    ) -> Option<Self::Instant> {
        timer.schedule(time).map(Into::into)
    }

    fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        timer.cancel().map(Into::into)
    }

    fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        timer.scheduled_time().map(Into::into)
    }

    fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId {
        timer.unique_id()
    }
}

impl DeviceLayerStateTypes for BindingsCtx {
    type LoopbackDeviceState = LoopbackInfo;
    type EthernetDeviceState = EthernetInfo;
    type PureIpDeviceState = PureIpDeviceInfo;
    type DeviceIdentifier = DeviceIdAndName;
}

impl ReceiveQueueBindingsContext<LoopbackDeviceId<Self>> for BindingsCtx {
    fn wake_rx_task(&mut self, device: &LoopbackDeviceId<Self>) {
        let LoopbackInfo { static_common_info: _, dynamic_common_info: _, rx_notifier } =
            device.external_state();
        rx_notifier.schedule()
    }
}

impl<D: DeviceIdExt> TransmitQueueBindingsContext<D> for BindingsCtx {
    fn wake_tx_task(&mut self, device: &D) {
        let netdevice = match device.external_state() {
            DeviceSpecificInfo::Loopback(_) => panic!("loopback does not support tx tasks"),
            DeviceSpecificInfo::Ethernet(EthernetInfo { netdevice, .. }) => netdevice,
            DeviceSpecificInfo::PureIp(PureIpDeviceInfo { netdevice, .. }) => netdevice,
        };
        netdevice.tx_notifier.schedule();
    }
}

impl DeviceLayerEventDispatcher for BindingsCtx {
    type DequeueContext = TxTaskState;

    fn send_ethernet_frame(
        &mut self,
        device: &EthernetDeviceId<Self>,
        frame: Buf<Vec<u8>>,
        dequeue_context: Option<&mut Self::DequeueContext>,
    ) -> Result<(), DeviceSendFrameError> {
        let EthernetInfo { mac: _, _mac_proxy: _, common_info: _, netdevice, dynamic_info } =
            device.external_state();
        let dynamic_info = dynamic_info.read();
        send_netdevice_frame(
            netdevice,
            &dynamic_info.netdevice,
            frame,
            fhardware_network::FrameType::Ethernet,
            dequeue_context,
        )
    }

    fn send_ip_packet(
        &mut self,
        device: &PureIpDeviceId<Self>,
        packet: Buf<Vec<u8>>,
        ip_version: IpVersion,
        dequeue_context: Option<&mut Self::DequeueContext>,
    ) -> Result<(), DeviceSendFrameError> {
        let frame_type = match ip_version {
            IpVersion::V4 => fhardware_network::FrameType::Ipv4,
            IpVersion::V6 => fhardware_network::FrameType::Ipv6,
        };
        let PureIpDeviceInfo { common_info: _, netdevice, dynamic_info } = device.external_state();
        let dynamic_info = dynamic_info.read();
        send_netdevice_frame(netdevice, &dynamic_info, packet, frame_type, dequeue_context)
    }
}

/// Send a frame on a Netdevice backed device.
fn send_netdevice_frame(
    netdevice: &StaticNetdeviceInfo,
    DynamicNetdeviceInfo {
        phy_up,
        common_info:
            DynamicCommonInfo { admin_enabled, mtu: _, events: _, control_hook: _, addresses: _ },
    }: &DynamicNetdeviceInfo,
    frame: Buf<Vec<u8>>,
    frame_type: fhardware_network::FrameType,
    dequeue_context: Option<&mut TxTaskState>,
) -> Result<(), DeviceSendFrameError> {
    let StaticNetdeviceInfo { handler, .. } = netdevice;
    if !(*phy_up && *admin_enabled) {
        debug!("dropped frame to {handler:?}, device offline");
        return Ok(());
    }

    let tx_buffer = match dequeue_context.and_then(|TxTaskState { tx_buffers }| tx_buffers.pop()) {
        Some(b) => b,
        None => {
            match handler.alloc_tx_buffer() {
                Ok(Some(b)) => b,
                Ok(None) => {
                    return Err(DeviceSendFrameError::NoBuffers);
                }
                Err(e) => {
                    error!("failed to allocate frame to {handler:?}: {e:?}");
                    // There's nothing core can do with this error, pretend like
                    // everything's okay.

                    // TODO(https://fxbug.dev/353718697): Consider signalling
                    // back through the handler that we want to shutdown this
                    // interface.
                    return Ok(());
                }
            }
        }
    };

    handler
        .send(frame.as_ref(), frame_type, tx_buffer)
        .unwrap_or_else(|e| warn!("failed to send frame to {:?}: {:?}", handler, e));
    Ok(())
}

impl<I: IpExt> IcmpEchoBindingsContext<I, DeviceId<BindingsCtx>> for BindingsCtx {
    fn receive_icmp_echo_reply<B: BufferMut>(
        &mut self,
        conn: &IcmpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>,
        device: &DeviceId<BindingsCtx>,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        id: u16,
        data: B,
    ) {
        conn.external_data().receive_icmp_echo_reply(device, src_ip, dst_ip, id, data)
    }
}

impl IcmpEchoBindingsTypes for BindingsCtx {
    type ExternalData<I: Ip> = socket::datagram::DatagramSocketExternalData<I>;
}

impl<I: IpExt> UdpReceiveBindingsContext<I, DeviceId<BindingsCtx>> for BindingsCtx {
    fn receive_udp<B: BufferMut>(
        &mut self,
        id: &UdpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>,
        device_id: &DeviceId<BindingsCtx>,
        meta: UdpPacketMeta<I>,
        body: &B,
    ) {
        id.external_data().receive_udp(device_id, meta, body)
    }
}

impl UdpBindingsTypes for BindingsCtx {
    type ExternalData<I: Ip> = socket::datagram::DatagramSocketExternalData<I>;
}

impl<I: Ip> EventContext<IpDeviceEvent<DeviceId<BindingsCtx>, I, StackTime>> for BindingsCtx {
    fn on_event(&mut self, event: IpDeviceEvent<DeviceId<BindingsCtx>, I, StackTime>) {
        match event {
            IpDeviceEvent::AddressAdded { device, addr, state, valid_until } => {
                let valid_until = valid_until.into_zx_time();

                self.notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressAdded {
                        addr: addr.into(),
                        assignment_state: state,
                        valid_until,
                    },
                );
                self.notify_address_update(&device, addr.addr().into(), state);
            }
            IpDeviceEvent::AddressRemoved { device, addr, reason } => {
                self.notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressRemoved(addr.to_ip_addr()),
                );
                match reason {
                    AddressRemovedReason::Manual => (),
                    AddressRemovedReason::DadFailed => self.notify_dad_failed(&device, addr.into()),
                }
            }
            IpDeviceEvent::AddressStateChanged { device, addr, state } => {
                self.notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressAssignmentStateChanged {
                        addr: addr.to_ip_addr(),
                        new_state: state,
                    },
                );
                self.notify_address_update(&device, addr.into(), state);
            }
            IpDeviceEvent::EnabledChanged { device, ip_enabled } => {
                self.notify_interface_update(&device, InterfaceUpdate::OnlineChanged(ip_enabled))
            }
            IpDeviceEvent::AddressPropertiesChanged { device, addr, valid_until } => self
                .notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressPropertiesChanged {
                        addr: addr.to_ip_addr(),
                        update: AddressPropertiesUpdate { valid_until: valid_until.into_zx_time() },
                    },
                ),
        };
    }
}

impl<I: Ip> EventContext<netstack3_core::ip::IpLayerEvent<DeviceId<BindingsCtx>, I>>
    for BindingsCtx
{
    fn on_event(&mut self, event: netstack3_core::ip::IpLayerEvent<DeviceId<BindingsCtx>, I>) {
        // Core dispatched a routes-change request but has no expectation of
        // observing the result, so we just discard the result receiver.
        match event {
            netstack3_core::ip::IpLayerEvent::AddRoute(entry) => {
                self.routes.fire_main_table_route_change_and_forget::<I>(routes::Change::RouteOp(
                    routes::RouteOp::Add(entry.map_device_id(|d| d.downgrade())),
                    routes::SetMembership::CoreNdp,
                ))
            }
            netstack3_core::ip::IpLayerEvent::RemoveRoutes { subnet, device, gateway } => {
                self.routes.fire_main_table_route_change_and_forget::<I>(routes::Change::RouteOp(
                    routes::RouteOp::RemoveMatching {
                        subnet,
                        device: device.downgrade(),
                        gateway,
                        metric: None,
                    },
                    routes::SetMembership::CoreNdp,
                ))
            }
        };
    }
}

impl<I: Ip> EventContext<neighbor::Event<Mac, EthernetDeviceId<Self>, I, StackTime>>
    for BindingsCtx
{
    fn on_event(
        &mut self,
        neighbor::Event { device, kind, addr, at }: neighbor::Event<
            Mac,
            EthernetDeviceId<Self>,
            I,
            StackTime,
        >,
    ) {
        device.external_state().with_dynamic_info(|i| {
            i.neighbor_event_sink
                .unbounded_send(neighbor_worker::Event {
                    id: device.downgrade(),
                    kind,
                    addr: addr.into(),
                    at,
                })
                .expect("should be able to send neighbor event")
        })
    }
}

/// Implements `RcNotifier` for futures oneshot channels.
///
/// We need a newtype here because of orphan rules.
pub(crate) struct ReferenceNotifier<T>(Option<futures::channel::oneshot::Sender<T>>);

impl<T: Send> netstack3_core::sync::RcNotifier<T> for ReferenceNotifier<T> {
    fn notify(&mut self, data: T) {
        let Self(inner) = self;
        inner.take().expect("notified twice").send(data).unwrap_or_else(|_: T| {
            panic!(
                "receiver was dropped before notifying for {}",
                // Print the type name so we don't need Debug bounds.
                core::any::type_name::<T>()
            )
        })
    }
}

pub(crate) struct ReferenceReceiver<T> {
    pub(crate) receiver: futures::channel::oneshot::Receiver<T>,
    pub(crate) debug_references: DynDebugReferences,
}

impl<T> ReferenceReceiver<T> {
    pub(crate) fn into_future<'a>(
        self,
        resource_name: &'static str,
        resource_id: &'a impl Debug,
    ) -> impl Future<Output = T> + 'a
    where
        T: 'a,
    {
        let Self { receiver, debug_references: refs } = self;
        debug!("{resource_name} {resource_id:?} removal is pending references: {refs:?}");
        // If we get stuck trying to remove the resource, log the remaining refs
        // at a low frequency to aid debugging.
        let interval_logging = fasync::Interval::new(zx::Duration::from_seconds(30))
            .map(move |()| {
                warn!("{resource_name} {resource_id:?} removal is pending references: {refs:?}")
            })
            .collect::<()>();

        futures::future::select(receiver, interval_logging).map(|r| match r {
            futures::future::Either::Left((rcv, _)) => {
                rcv.expect("sender dropped without notifying")
            }
            futures::future::Either::Right(((), _)) => {
                unreachable!("interval channel never completes")
            }
        })
    }
}

impl ReferenceNotifiers for BindingsCtx {
    type ReferenceReceiver<T: 'static> = ReferenceReceiver<T>;

    type ReferenceNotifier<T: Send + 'static> = ReferenceNotifier<T>;

    fn new_reference_notifier<T: Send + 'static>(
        debug_references: DynDebugReferences,
    ) -> (Self::ReferenceNotifier<T>, Self::ReferenceReceiver<T>) {
        let (sender, receiver) = futures::channel::oneshot::channel();
        (ReferenceNotifier(Some(sender)), ReferenceReceiver { receiver, debug_references })
    }
}

impl DeferredResourceRemovalContext for BindingsCtx {
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn defer_removal<T: Send + 'static>(&mut self, receiver: Self::ReferenceReceiver<T>) {
        let ReferenceReceiver { receiver, debug_references } = receiver;
        self.resource_removal.defer_removal(
            debug_references,
            receiver.map(|r| r.expect("sender dropped without notifying receiver")),
        );
    }
}

impl BindingsCtx {
    fn notify_interface_update(&self, device: &DeviceId<BindingsCtx>, event: InterfaceUpdate) {
        device
            .external_state()
            .with_common_info(|i| i.events.notify(event).expect("interfaces worker closed"));
    }

    /// Notify `AddressStateProvider.WatchAddressAssignmentState` watchers.
    fn notify_address_update(
        &self,
        device: &DeviceId<BindingsCtx>,
        address: SpecifiedAddr<IpAddr>,
        state: netstack3_core::ip::IpAddressState,
    ) {
        // Note that not all addresses have an associated watcher (e.g. loopback
        // address & autoconfigured SLAAC addresses).
        device.external_state().with_common_info(|i| {
            if let Some(address_info) = i.addresses.get(&address) {
                address_info
                    .assignment_state_sender
                    .unbounded_send(state.into_fidl())
                    .expect("assignment state receiver unexpectedly disconnected");
            }
        })
    }

    fn notify_dad_failed(
        &mut self,
        device: &DeviceId<BindingsCtx>,
        address: SpecifiedAddr<IpAddr>,
    ) {
        device.external_state().with_common_info_mut(|i| {
            if let Some(address_info) = i.addresses.get_mut(&address) {
                let devices::FidlWorkerInfo { worker: _, cancelation_sender } =
                    &mut address_info.address_state_provider;
                if let Some(sender) = cancelation_sender.take() {
                    sender
                        .send(interfaces_admin::AddressStateProviderCancellationReason::DadFailed)
                        .expect("assignment state receiver unexpectedly disconnected");
                }
            }
        })
    }

    pub(crate) async fn apply_route_change<I: Ip>(
        &self,
        change: routes::Change<I>,
    ) -> Result<routes::ChangeOutcome, routes::ChangeError> {
        self.routes.send_main_table_route_change(change).await
    }

    pub(crate) async fn apply_route_change_either(
        &self,
        change: routes::ChangeEither,
    ) -> Result<routes::ChangeOutcome, routes::ChangeError> {
        match change {
            routes::ChangeEither::V4(change) => self.apply_route_change::<Ipv4>(change).await,
            routes::ChangeEither::V6(change) => self.apply_route_change::<Ipv6>(change).await,
        }
    }

    pub(crate) async fn remove_routes_on_device(
        &self,
        device: &netstack3_core::device::WeakDeviceId<Self>,
    ) {
        match self
            .apply_route_change::<Ipv4>(routes::Change::RemoveMatchingDevice(device.clone()))
            .await
            .expect("deleting routes on device during removal should succeed")
        {
            routes::ChangeOutcome::Changed | routes::ChangeOutcome::NoChange => {
                // We don't care whether there were any routes on the device or not.
            }
        }
        match self
            .apply_route_change::<Ipv6>(routes::Change::RemoveMatchingDevice(device.clone()))
            .await
            .expect("deleting routes on device during removal should succeed")
        {
            routes::ChangeOutcome::Changed | routes::ChangeOutcome::NoChange => {
                // We don't care whether there were any routes on the device or not.
            }
        }
    }
}

fn add_loopback_ip_addrs(
    ctx: &mut Ctx,
    loopback: &DeviceId<BindingsCtx>,
) -> Result<(), ExistsError> {
    for addr_subnet in [
        AddrSubnetEither::V4(
            AddrSubnet::from_witness(Ipv4::LOOPBACK_ADDRESS, Ipv4::LOOPBACK_SUBNET.prefix())
                .expect("error creating IPv4 loopback AddrSub"),
        ),
        AddrSubnetEither::V6(
            AddrSubnet::from_witness(Ipv6::LOOPBACK_ADDRESS, Ipv6::LOOPBACK_SUBNET.prefix())
                .expect("error creating IPv6 loopback AddrSub"),
        ),
    ] {
        ctx.api().device_ip_any().add_ip_addr_subnet(loopback, addr_subnet).map_err(
            |e| match e {
                AddIpAddrSubnetError::Exists => ExistsError,
                AddIpAddrSubnetError::InvalidAddr => {
                    panic!("loopback address should not be invalid")
                }
            },
        )?
    }
    Ok(())
}

/// Adds the IPv4 and IPv6 Loopback and multicast subnet routes, and the IPv4
/// limited broadcast subnet route.
async fn add_loopback_routes(bindings_ctx: &BindingsCtx, loopback: &DeviceId<BindingsCtx>) {
    use netstack3_core::routes::{AddableEntry, AddableMetric};

    let v4_changes = [
        AddableEntry::without_gateway(
            Ipv4::LOOPBACK_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
        AddableEntry::without_gateway(
            Ipv4::MULTICAST_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
    ]
    .into_iter()
    .map(|entry| {
        routes::Change::<Ipv4>::RouteOp(
            routes::RouteOp::Add(entry),
            routes::SetMembership::Loopback,
        )
    })
    .map(Into::into);

    let v6_changes = [
        AddableEntry::without_gateway(
            Ipv6::LOOPBACK_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
        AddableEntry::without_gateway(
            Ipv6::MULTICAST_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
    ]
    .into_iter()
    .map(|entry| {
        routes::Change::<Ipv6>::RouteOp(
            routes::RouteOp::Add(entry),
            routes::SetMembership::Loopback,
        )
    })
    .map(Into::into);

    for change in v4_changes.chain(v6_changes) {
        bindings_ctx
            .apply_route_change_either(change)
            .await
            .map(|outcome| assert_matches!(outcome, routes::ChangeOutcome::Changed))
            .expect("adding loopback routes should succeed");
    }
}

/// The netstack.
///
/// Provides the entry point for creating a netstack to be served as a
/// component.
#[derive(Clone)]
pub(crate) struct Netstack {
    ctx: Ctx,
    interfaces_event_sink: interfaces_watcher::WorkerInterfaceSink,
    neighbor_event_sink: mpsc::UnboundedSender<neighbor_worker::Event>,
}

fn create_interface_event_producer(
    interfaces_event_sink: &crate::bindings::interfaces_watcher::WorkerInterfaceSink,
    id: BindingId,
    properties: InterfaceProperties,
) -> InterfaceEventProducer {
    interfaces_event_sink.add_interface(id, properties).expect("interface worker not running")
}

impl Netstack {
    fn create_interface_event_producer(
        &self,
        id: BindingId,
        properties: InterfaceProperties,
    ) -> InterfaceEventProducer {
        create_interface_event_producer(&self.interfaces_event_sink, id, properties)
    }

    async fn add_loopback(
        &mut self,
    ) -> (
        futures::channel::oneshot::Sender<fnet_interfaces_admin::InterfaceRemovedReason>,
        BindingId,
        [NamedTask; 2],
    ) {
        // Add and initialize the loopback interface with the IPv4 and IPv6
        // loopback addresses and on-link routes to the loopback subnets.
        let devices: &Devices<_> = self.ctx.bindings_ctx().as_ref();
        let (control_sender, control_receiver) =
            interfaces_admin::OwnedControlHandle::new_channel();
        let loopback_rx_notifier = Default::default();

        let binding_id = devices.alloc_new_id();
        let events = self.create_interface_event_producer(
            binding_id,
            InterfaceProperties {
                name: LOOPBACK_NAME.to_string(),
                port_class: fidl_fuchsia_net_interfaces_ext::PortClass::Loopback,
            },
        );
        events.notify(InterfaceUpdate::OnlineChanged(true)).expect("interfaces worker not running");

        let loopback_info = LoopbackInfo {
            static_common_info: StaticCommonInfo { authorization_token: zx::Event::create() },
            dynamic_common_info: CoreRwLock::new(DynamicCommonInfo {
                mtu: DEFAULT_LOOPBACK_MTU,
                admin_enabled: true,
                events,
                control_hook: control_sender,
                addresses: HashMap::new(),
            }),
            rx_notifier: loopback_rx_notifier,
        };

        let loopback = self.ctx.api().device::<LoopbackDevice>().add_device(
            DeviceIdAndName { id: binding_id, name: LOOPBACK_NAME.to_string() },
            LoopbackCreationProperties { mtu: DEFAULT_LOOPBACK_MTU },
            RawMetric(DEFAULT_INTERFACE_METRIC),
            loopback_info,
        );

        let LoopbackInfo { static_common_info: _, dynamic_common_info: _, rx_notifier } =
            loopback.external_state();
        let rx_task =
            crate::bindings::devices::spawn_rx_task(rx_notifier, self.ctx.clone(), &loopback);
        let binding_id = loopback.bindings_id().id;
        let loopback: DeviceId<_> = loopback.into();
        self.ctx.bindings_ctx().devices.add_device(binding_id, loopback.clone());

        // Don't need DAD and IGMP/MLD on loopback.
        let ip_config = IpDeviceConfigurationUpdate {
            ip_enabled: Some(true),
            unicast_forwarding_enabled: Some(false),
            multicast_forwarding_enabled: Some(false),
            gmp_enabled: Some(false),
        };

        let _: Ipv4DeviceConfigurationUpdate = self
            .ctx
            .api()
            .device_ip::<Ipv4>()
            .update_configuration(&loopback, Ipv4DeviceConfigurationUpdate { ip_config })
            .unwrap();
        let _: Ipv6DeviceConfigurationUpdate = self
            .ctx
            .api()
            .device_ip::<Ipv6>()
            .update_configuration(
                &loopback,
                Ipv6DeviceConfigurationUpdate {
                    dad_transmits: Some(None),
                    max_router_solicitations: Some(None),
                    slaac_config: Some(SlaacConfiguration {
                        enable_stable_addresses: true,
                        temporary_address_configuration: None,
                    }),
                    ip_config,
                },
            )
            .unwrap();
        add_loopback_ip_addrs(&mut self.ctx, &loopback).expect("error adding loopback addresses");
        add_loopback_routes(self.ctx.bindings_ctx(), &loopback).await;

        let (stop_sender, stop_receiver) = futures::channel::oneshot::channel();

        // Loopback interface can't be removed.
        let removable = false;
        // Loopback doesn't have a defined state stream, provide a stream that
        // never yields anything.
        let state_stream = futures::stream::pending();
        let control_task = fuchsia_async::Task::spawn(interfaces_admin::run_interface_control(
            self.ctx.clone(),
            binding_id,
            stop_receiver,
            control_receiver,
            removable,
            state_stream,
            || (),
        ));
        (
            stop_sender,
            binding_id,
            [
                NamedTask::new("loopback control", control_task),
                NamedTask::new("loopback rx", rx_task),
            ],
        )
    }
}

pub(crate) enum Service {
    DnsServerWatcher(fidl_fuchsia_net_name::DnsServerWatcherRequestStream),
    DebugDiagnostics(fidl::endpoints::ServerEnd<fidl_fuchsia_net_debug::DiagnosticsMarker>),
    DebugInterfaces(fidl_fuchsia_net_debug::InterfacesRequestStream),
    FilterControl(fidl_fuchsia_net_filter::ControlRequestStream),
    FilterState(fidl_fuchsia_net_filter::StateRequestStream),
    Interfaces(fidl_fuchsia_net_interfaces::StateRequestStream),
    InterfacesAdmin(fidl_fuchsia_net_interfaces_admin::InstallerRequestStream),
    MulticastAdminV4(fidl_fuchsia_net_multicast_admin::Ipv4RoutingTableControllerRequestStream),
    MulticastAdminV6(fidl_fuchsia_net_multicast_admin::Ipv6RoutingTableControllerRequestStream),
    NeighborController(fidl_fuchsia_net_neighbor::ControllerRequestStream),
    Neighbor(fidl_fuchsia_net_neighbor::ViewRequestStream),
    PacketSocket(fidl_fuchsia_posix_socket_packet::ProviderRequestStream),
    RawSocket(fidl_fuchsia_posix_socket_raw::ProviderRequestStream),
    RootFilter(fidl_fuchsia_net_root::FilterRequestStream),
    RootInterfaces(fidl_fuchsia_net_root::InterfacesRequestStream),
    RootRoutesV4(fidl_fuchsia_net_root::RoutesV4RequestStream),
    RootRoutesV6(fidl_fuchsia_net_root::RoutesV6RequestStream),
    RoutesState(fidl_fuchsia_net_routes::StateRequestStream),
    RoutesStateV4(fidl_fuchsia_net_routes::StateV4RequestStream),
    RoutesStateV6(fidl_fuchsia_net_routes::StateV6RequestStream),
    RoutesAdminV4(fnet_routes_admin::RouteTableV4RequestStream),
    RoutesAdminV6(fnet_routes_admin::RouteTableV6RequestStream),
    RouteTableProviderV4(fnet_routes_admin::RouteTableProviderV4RequestStream),
    RouteTableProviderV6(fnet_routes_admin::RouteTableProviderV6RequestStream),
    RuleTableV4(fnet_routes_admin::RuleTableV4RequestStream),
    RuleTableV6(fnet_routes_admin::RuleTableV6RequestStream),
    Socket(fidl_fuchsia_posix_socket::ProviderRequestStream),
    Stack(fidl_fuchsia_net_stack::StackRequestStream),
    Verifier(fidl_fuchsia_update_verify::NetstackVerifierRequestStream),
}

trait RequestStreamExt: RequestStream {
    fn serve_with<F, Fut, E>(self, f: F) -> futures::future::Map<Fut, fn(Result<(), E>) -> ()>
    where
        E: std::error::Error,
        F: FnOnce(Self) -> Fut,
        Fut: Future<Output = Result<(), E>>;
}

impl<D: DiscoverableProtocolMarker, S: RequestStream<Protocol = D>> RequestStreamExt for S {
    fn serve_with<F, Fut, E>(self, f: F) -> futures::future::Map<Fut, fn(Result<(), E>) -> ()>
    where
        E: std::error::Error,
        F: FnOnce(Self) -> Fut,
        Fut: Future<Output = Result<(), E>>,
    {
        f(self).map(|res| res.unwrap_or_else(|err| error!("{} error: {}", D::PROTOCOL_NAME, err)))
    }
}

/// A helper struct to have named tasks.
///
/// Tasks are already tracked in the executor by spawn location, but long
/// running tasks are not expected to terminate except during clean shutdown.
/// Naming these helps root cause debug assertions.
#[derive(Debug)]
pub(crate) struct NamedTask {
    name: &'static str,
    task: fuchsia_async::Task<()>,
}

impl NamedTask {
    /// Creates a new named task from `fut` with `name`.
    #[track_caller]
    fn spawn(name: &'static str, fut: impl futures::Future<Output = ()> + Send + 'static) -> Self {
        Self { name, task: fuchsia_async::Task::spawn(fut) }
    }

    fn new(name: &'static str, task: fuchsia_async::Task<()>) -> Self {
        Self { name, task }
    }

    fn into_future(self) -> impl futures::Future<Output = &'static str> + Send + 'static {
        let Self { name, task } = self;
        task.map(move |()| name)
    }
}

impl NetstackSeed {
    /// Consumes the netstack and starts serving all the FIDL services it
    /// implements to the outgoing service directory.
    pub(crate) async fn serve<S: futures::Stream<Item = Service>>(
        self,
        services: S,
        inspect_publisher: InspectPublisher<'_>,
    ) {
        let Self {
            mut netstack,
            interfaces_worker,
            interfaces_watcher_sink,
            mut routes_change_runner,
            neighbor_worker,
            neighbor_watcher_sink,
            resource_removal_worker,
        } = self;

        // Start servicing timers.
        let mut timer_handler_ctx = netstack.ctx.clone();
        let timers_task = NamedTask::new(
            "timers",
            netstack.ctx.bindings_ctx().timers.spawn(move |dispatch, timer| {
                timer_handler_ctx.api().handle_timer(dispatch, timer);
            }),
        );

        let (dispatchers_v4, dispatchers_v6) = routes_change_runner.update_dispatchers();

        // Start executing routes changes.
        let routes_change_task = NamedTask::spawn("routes_changes", {
            let ctx = netstack.ctx.clone();
            async move { routes_change_runner.run(ctx).await }
        });

        let routes_change_task_fut = routes_change_task.into_future().fuse();
        let mut routes_change_task_fut = pin!(routes_change_task_fut);

        // Start executing delayed resource removal.
        let resource_removal_task =
            NamedTask::spawn("resource_removal", resource_removal_worker.run());
        let resource_removal_task_fut = resource_removal_task.into_future().fuse();
        let mut resource_removal_task_fut = pin!(resource_removal_task_fut);

        let (loopback_stopper, _, loopback_tasks): (
            futures::channel::oneshot::Sender<_>,
            BindingId,
            _,
        ) = netstack.add_loopback().await;

        let interfaces_worker_task = NamedTask::spawn("interfaces worker", async move {
            let result = interfaces_worker.run().await;
            let watchers = result.expect("interfaces worker ended with an error");
            info!("interfaces worker shutting down, waiting for watchers to end");
            watchers
                .map(|res| match res {
                    Ok(()) => (),
                    Err(e) => {
                        if !e.is_closed() {
                            error!("error {e:?} collecting watchers");
                        }
                    }
                })
                .collect::<()>()
                .await;
            info!("all interface watchers closed, interfaces worker shutdown is complete");
        });

        let neighbor_worker_task = NamedTask::spawn("neighbor worker", neighbor_worker.run());

        let no_finish_tasks = loopback_tasks.into_iter().chain([
            interfaces_worker_task,
            timers_task,
            neighbor_worker_task,
        ]);
        let mut no_finish_tasks = futures::stream::FuturesUnordered::from_iter(
            no_finish_tasks.map(NamedTask::into_future),
        );

        let unexpected_early_finish_fut = async {
            let no_finish_tasks_fut = no_finish_tasks.by_ref().next().fuse();
            let mut no_finish_tasks_fut = pin!(no_finish_tasks_fut);

            let name = select! {
                name = no_finish_tasks_fut => name,
                name = routes_change_task_fut => Some(name),
                name = resource_removal_task_fut => Some(name),
            };
            match name {
                Some(name) => panic!("task {name} ended unexpectedly"),
                None => panic!("unexpected end of infinite task stream"),
            }
        }
        .fuse();

        let inspector = inspect_publisher.inspector();
        let inspect_nodes = {
            // The presence of the health check node is useful even though the
            // status will always be OK because the same node exists
            // in NS2 and this helps for test assertions to guard against
            // issues such as https://fxbug.dev/326510415.
            let mut health = fuchsia_inspect::health::Node::new(inspector.root());
            health.set_ok();
            let socket_ctx = netstack.ctx.clone();
            let sockets = inspector.root().create_lazy_child("Sockets", move || {
                futures::future::ok(inspect::sockets(&mut socket_ctx.clone())).boxed()
            });
            let routes_ctx = netstack.ctx.clone();
            let routes = inspector.root().create_lazy_child("Routes", move || {
                futures::future::ok(inspect::routes(&mut routes_ctx.clone())).boxed()
            });
            let devices_ctx = netstack.ctx.clone();
            let devices = inspector.root().create_lazy_child("Devices", move || {
                futures::future::ok(inspect::devices(&mut devices_ctx.clone())).boxed()
            });
            let neighbors_ctx = netstack.ctx.clone();
            let neighbors = inspector.root().create_lazy_child("Neighbors", move || {
                futures::future::ok(inspect::neighbors(neighbors_ctx.clone())).boxed()
            });
            let counters_ctx = netstack.ctx.clone();
            let counters = inspector.root().create_lazy_child("Counters", move || {
                futures::future::ok(inspect::counters(&mut counters_ctx.clone())).boxed()
            });
            let filter_ctx = netstack.ctx.clone();
            let filtering_state =
                inspector.root().create_lazy_child("Filtering State", move || {
                    futures::future::ok(inspect::filtering_state(&mut filter_ctx.clone())).boxed()
                });
            (health, sockets, routes, devices, neighbors, counters, filtering_state)
        };

        let diagnostics_handler = debug_fidl_worker::DiagnosticsHandler::default();

        // Insert a stream after services to get a helpful log line when it
        // completes. The future we create from it will still wait for all the
        // user-created resources to be joined on before returning.
        let services =
            services.chain(futures::stream::poll_fn(|_: &mut std::task::Context<'_>| {
                info!("services stream ended");
                std::task::Poll::Ready(None)
            }));

        // Keep a clone of Ctx around for teardown before moving it to the
        // services future.
        let teardown_ctx = netstack.ctx.clone();

        // Use a reference to the watcher sink in the services loop.
        let interfaces_watcher_sink_ref = &interfaces_watcher_sink;
        let neighbor_watcher_sink_ref = &neighbor_watcher_sink;

        let (route_waitgroup, route_spawner) = TaskWaitGroup::new();

        let filter_update_dispatcher = filter::UpdateDispatcher::default();

        // It is unclear why we need to wrap the `for_each_concurrent` call with
        // `async move { ... }` but it seems like we do. Without this, the
        // `Future` returned by this function fails to implement `Send` with the
        // same issue reported in https://github.com/rust-lang/rust/issues/64552.
        //
        // TODO(https://github.com/rust-lang/rust/issues/64552): Remove this
        // workaround.
        let services_fut = async move {
            services
                .for_each_concurrent(None, |s| async {
                    match s {
                        Service::Stack(stack) => {
                            stack
                                .serve_with(|rs| {
                                    stack_fidl_worker::StackFidlWorker::serve(netstack.clone(), rs)
                                })
                                .await
                        }
                        Service::Socket(socket) => {
                            // Run on a separate task so socket requests are not
                            // bound to the same thread as the main services
                            // loop.
                            let wait_group = fuchsia_async::Task::spawn(socket::serve(
                                netstack.ctx.clone(),
                                socket,
                            ))
                            .await;
                            // Wait for all socket tasks to finish.
                            wait_group.await;
                        }
                        Service::PacketSocket(socket) => {
                            // Run on a separate task so socket requests are not
                            // bound to the same thread as the main services
                            // loop.
                            let wait_group = fuchsia_async::Task::spawn(socket::packet::serve(
                                netstack.ctx.clone(),
                                socket,
                            ))
                            .await;
                            // Wait for all socket tasks to finish.
                            wait_group.await;
                        }
                        Service::RawSocket(socket) => {
                            // Run on a separate task so socket requests are not
                            // bound to the same thread as the main services
                            // loop.
                            let wait_group = fuchsia_async::Task::spawn(socket::raw::serve(
                                netstack.ctx.clone(),
                                socket,
                            ))
                            .await;
                            // Wait for all socket tasks to finish.
                            wait_group.await;
                        }
                        Service::RootInterfaces(root_interfaces) => {
                            root_interfaces
                                .serve_with(|rs| {
                                    root_fidl_worker::serve_interfaces(netstack.clone(), rs)
                                })
                                .await
                        }
                        Service::RootFilter(root_filter) => {
                            root_filter
                                .serve_with(|rs|
                                    filter::serve_root(
                                        rs,
                                        &filter_update_dispatcher,
                                        &netstack.ctx,
                                    )
                                )
                                .await
                        }
                        Service::RoutesState(rs) => {
                            routes::state::serve_state(rs, netstack.ctx.clone()).await
                        }
                        Service::RoutesStateV4(rs) => {
                            routes::state::serve_state_v4(rs, &dispatchers_v4).await
                        }
                        Service::RoutesStateV6(rs) => {
                            routes::state::serve_state_v6(rs, &dispatchers_v6).await
                        }
                        Service::RoutesAdminV4(rs) => routes::admin::serve_route_table::<
                            Ipv4,
                            routes::admin::MainRouteTable,
                            _,
                        >(
                            rs,
                            route_spawner.clone(),
                            routes::admin::MainRouteTable::new(netstack.ctx.clone()),
                        )
                        .await,
                        Service::RoutesAdminV6(rs) => routes::admin::serve_route_table::<
                            Ipv6,
                            routes::admin::MainRouteTable,
                            _,
                        >(
                            rs,
                            route_spawner.clone(),
                            routes::admin::MainRouteTable::new(netstack.ctx.clone()),
                        )
                        .await,
                        Service::RouteTableProviderV4(stream) => {
                            routes::admin::serve_route_table_provider_v4(
                                stream,
                                route_spawner.clone(),
                                &netstack.ctx,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                error!(
                                    "error serving {}: {e:?}",
                                    fnet_routes_admin::RouteTableProviderV4Marker::DEBUG_NAME
                                );
                            })
                        }
                        Service::RouteTableProviderV6(stream) => {
                            routes::admin::serve_route_table_provider_v6(
                                stream,
                                route_spawner.clone(),
                                &netstack.ctx,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                error!(
                                    "error serving {}: {e:?}",
                                    fnet_routes_admin::RouteTableProviderV6Marker::DEBUG_NAME
                                );
                            })
                        }
                        Service::RuleTableV4(rule_table) => {
                            rule_table
                                .serve_with(|rs| {
                                    routes::admin::serve_rule_table::<Ipv4>(
                                        rs,
                                        route_spawner.clone(),
                                        &netstack.ctx,
                                    )
                                })
                                .await
                        }
                        Service::RuleTableV6(rule_table) => {
                            rule_table
                                .serve_with(|rs| {
                                    routes::admin::serve_rule_table::<Ipv6>(
                                        rs,
                                        route_spawner.clone(),
                                        &netstack.ctx,
                                    )
                                })
                                .await
                        }
                        Service::RootRoutesV4(rs) => root_fidl_worker::serve_routes_v4(
                            rs,
                            route_spawner.clone(),
                            &netstack.ctx,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            error!(
                                "error serving {}: {e:?}",
                                fidl_fuchsia_net_root::RoutesV4Marker::DEBUG_NAME
                            );
                        }),
                        Service::RootRoutesV6(rs) => root_fidl_worker::serve_routes_v6(
                            rs,
                            route_spawner.clone(),
                            &netstack.ctx,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            error!(
                                "error serving {}: {e:?}",
                                fidl_fuchsia_net_root::RoutesV6Marker::DEBUG_NAME
                            );
                        }),
                        Service::Interfaces(interfaces) => {
                            interfaces
                                .serve_with(|rs| {
                                    interfaces_watcher::serve(
                                        rs,
                                        interfaces_watcher_sink_ref.clone(),
                                    )
                                })
                                .await
                        }
                        Service::InterfacesAdmin(installer) => {
                            debug!(
                                "serving {}",
                                fidl_fuchsia_net_interfaces_admin::InstallerMarker::PROTOCOL_NAME
                            );
                            interfaces_admin::serve(netstack.clone(), installer).await;
                        }
                        Service::MulticastAdminV4(controller) => {
                            debug!(
                                "serving {}",
                                fnet_multicast_admin::Ipv4RoutingTableControllerMarker::PROTOCOL_NAME
                            );
                            multicast_admin::serve_table_controller::<Ipv4>(
                                netstack.ctx.clone(),
                                controller
                            ).await;
                        }
                        Service::MulticastAdminV6(controller) => {
                            debug!(
                                "serving {}",
                                fnet_multicast_admin::Ipv6RoutingTableControllerMarker::PROTOCOL_NAME
                            );
                            multicast_admin::serve_table_controller::<Ipv6>(
                                netstack.ctx.clone(),
                                controller
                            ).await;
                        }
                        Service::DebugInterfaces(debug_interfaces) => {
                            debug_interfaces
                                .serve_with(|rs| {
                                    debug_fidl_worker::serve_interfaces(
                                        netstack.ctx.bindings_ctx(),
                                        rs,
                                    )
                                })
                                .await
                        }
                        Service::DebugDiagnostics(debug_diagnostics) => {
                            diagnostics_handler.serve_diagnostics(debug_diagnostics).await
                        }
                        Service::DnsServerWatcher(dns) => {
                            dns.serve_with(|rs| name_worker::serve(netstack.clone(), rs)).await
                        }
                        Service::FilterState(filter) => {
                            filter
                                .serve_with(|rs| filter::serve_state(rs, &filter_update_dispatcher))
                                .await
                        }
                        Service::FilterControl(filter) => {
                            filter
                                .serve_with(|rs| {
                                    filter::serve_control(
                                        rs,
                                        &filter_update_dispatcher,
                                        &netstack.ctx,
                                    )
                                })
                                .await
                        }
                        Service::Neighbor(neighbor) => {
                            neighbor
                                .serve_with(|rs| {
                                    neighbor_worker::serve_view(
                                        rs,
                                        neighbor_watcher_sink_ref.clone(),
                                    )
                                })
                                .await
                        }
                        Service::NeighborController(neighbor_controller) => {
                            neighbor_controller
                                .serve_with(|rs| {
                                    neighbor_worker::serve_controller(netstack.ctx.clone(), rs)
                                })
                                .await
                        }
                        Service::Verifier(verifier) => {
                            verifier.serve_with(|rs| verifier_worker::serve(rs)).await
                        }
                    }
                })
                .await
        };

        // We just let this be destroyed on drop because it's effectively tied
        // to the lifecycle of the entire component.
        let _inspect_task = inspect_publisher.publish();

        {
            let services_fut = services_fut.fuse();
            // Pin services_fut to this block scope so it's dropped after the
            // select.
            let mut services_fut = pin!(services_fut);

            // Do likewise for unexpected_early_finish_fut.
            let mut unexpected_early_finish_fut = pin!(unexpected_early_finish_fut);

            let () = futures::select! {
                () = services_fut => (),
                never = unexpected_early_finish_fut => {
                    let never: Never = never;
                    match never {}
                },
            };
        }

        info!("all services terminated, starting shutdown");
        let ctx = teardown_ctx;
        // Stop the loopback interface.
        loopback_stopper
            .send(fnet_interfaces_admin::InterfaceRemovedReason::PortClosed)
            .expect("loopback task must still be running");
        // Stop the timer dispatcher.
        ctx.bindings_ctx().timers.stop();
        // Stop the interfaces watcher worker.
        std::mem::drop(interfaces_watcher_sink);
        // Stop the neighbor watcher worker.
        std::mem::drop(neighbor_watcher_sink);

        // Collect the routes admin waitgroup.
        route_waitgroup.await;

        // We've signalled all long running tasks, now we can collect them.
        no_finish_tasks.map(|name| info!("{name} finished")).collect::<()>().await;

        // Stop the routes change runner.
        ctx.bindings_ctx().routes.close_senders();
        let _task_name: &str = routes_change_task_fut.await;

        // Stop the resource removal worker.
        ctx.bindings_ctx().resource_removal.close();
        let _task_name: &str = resource_removal_task_fut.await;

        // Drop all inspector data, it holds ctx clones.
        std::mem::drop(inspect_nodes);
        inspector.root().clear_recorded();

        // Last thing to happen is dropping the context.
        ctx.try_destroy_last().expect("all Ctx references must have been dropped")
    }
}
