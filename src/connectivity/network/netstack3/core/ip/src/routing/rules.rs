// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IP routing rules.

use alloc::vec::Vec;
use core::fmt::Debug;
use core::ops::Deref as _;

use net_types::ip::Ip;
use netstack3_base::{DeviceNameMatcher, DeviceWithName, Matcher, SubnetMatcher};

use crate::internal::routing::PacketOrigin;
use crate::RoutingTableId;

/// Table that contains routing rules.
pub struct RulesTable<I: Ip, D> {
    rules: Vec<Rule<I, D>>,
}

impl<I: Ip, D> RulesTable<I, D> {
    pub(crate) fn new(main_table_id: RoutingTableId<I, D>) -> Self {
        // TODO(https://fxbug.dev/355059790): If bindings is installing the main table, we should
        // also let the bindings install this default rule.
        Self {
            rules: alloc::vec![Rule {
                matcher: RuleMatcher::match_all_packets(),
                action: RuleAction::Lookup(main_table_id)
            }],
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'_ Rule<I, D>> {
        self.rules.iter()
    }

    #[cfg(test)]
    pub(crate) fn rules_mut(&mut self) -> &mut Vec<Rule<I, D>> {
        &mut self.rules
    }
}

/// A routing rule.
pub(crate) struct Rule<I: Ip, D> {
    pub(crate) matcher: RuleMatcher<I>,
    pub(crate) action: RuleAction<RoutingTableId<I, D>>,
}

/// The action part of a [`Rule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuleAction<Lookup> {
    /// Will resolve to unreachable.
    // TODO(https://fxbug.dev/357858471): Install Bindings rules in Core.
    #[allow(unused)]
    Unreachable,
    /// Lookup in a routing table.
    Lookup(Lookup),
}

/// Matches with [`PacketOrigin`].
///
/// Note that this matcher doesn't specify the source address/bound address like [`PacketOrigin`]
/// because the user can specify a source address matcher without specifying the direction of the
/// traffic.
#[derive(Debug, Clone)]
pub(crate) enum TrafficOriginMatcher {
    /// This only matches packets that are generated locally; the optional interface matcher
    /// can be used to match what device is bound to by `SO_BINDTODEVICE`.
    // TODO(https://fxbug.dev/357858471): Install Bindings rules in Core.
    #[allow(unused)]
    Local { bound_device_matcher: Option<DeviceNameMatcher> },
    /// This only matches non-local packets. The packets must be received from the network.
    // TODO(https://fxbug.dev/357858471): Install Bindings rules in Core.
    #[allow(unused)]
    NonLocal,
}

impl<'a, I: Ip, D: DeviceWithName> Matcher<PacketOrigin<I, &'a D>> for SubnetMatcher<I::Addr> {
    fn matches(&self, actual: &PacketOrigin<I, &'a D>) -> bool {
        match actual {
            PacketOrigin::Local { bound_address, bound_device: _ } => {
                self.required_matches(bound_address.as_deref())
            }
            PacketOrigin::NonLocal { source_address, incoming_device: _ } => {
                self.matches(source_address.deref())
            }
        }
    }
}

impl<'a, I: Ip, D: DeviceWithName> Matcher<PacketOrigin<I, &'a D>> for TrafficOriginMatcher {
    fn matches(&self, actual: &PacketOrigin<I, &'a D>) -> bool {
        match (self, actual) {
            (
                TrafficOriginMatcher::Local { bound_device_matcher },
                PacketOrigin::Local { bound_address: _, bound_device },
            ) => bound_device_matcher.required_matches(*bound_device),
            (
                TrafficOriginMatcher::NonLocal,
                PacketOrigin::NonLocal { source_address: _, incoming_device: _ },
            ) => true,
            (TrafficOriginMatcher::Local { .. }, PacketOrigin::NonLocal { .. })
            | (TrafficOriginMatcher::NonLocal, PacketOrigin::Local { .. }) => false,
        }
    }
}

/// Contains traffic matchers for a given rule.
///
/// `None` fields match all packets.
#[derive(Debug, Clone)]
pub(crate) struct RuleMatcher<I: Ip> {
    /// Matches on [`PacketOrigin`]'s bound address for a locally generated packet or the source
    /// address of an incoming packet.
    ///
    /// Matches whether the source address of the packet is from the subnet. If the matcher is
    /// specified but the source address is not specified, it resolves to not a match.
    pub(crate) source_address_matcher: Option<SubnetMatcher<I::Addr>>,
    /// Matches on [`PacketOrigin`]'s bound device for a locally generated packets or the receiving
    /// device of an incoming packet.
    pub(crate) traffic_origin_matcher: Option<TrafficOriginMatcher>,
    // TODO(https://fxbug.dev/337134565): Implement socket marks.
}

impl<I: Ip> RuleMatcher<I> {
    /// Creates a rule matcher that matches all packets.
    pub(crate) fn match_all_packets() -> Self {
        RuleMatcher { source_address_matcher: None, traffic_origin_matcher: None }
    }
}

/// Packet properties used as input for the rules engine.
pub struct RuleInput<'a, I: Ip, D> {
    pub(crate) packet_origin: PacketOrigin<I, &'a D>,
}

impl<'a, I: Ip, D: DeviceWithName> Matcher<RuleInput<'a, I, D>> for RuleMatcher<I> {
    fn matches(&self, actual: &RuleInput<'a, I, D>) -> bool {
        let Self { source_address_matcher, traffic_origin_matcher } = self;
        let RuleInput { packet_origin } = actual;
        source_address_matcher.matches(packet_origin)
            && traffic_origin_matcher.matches(packet_origin)
    }
}

#[cfg(test)]
mod test {
    use ip_test_macro::ip_test;
    use net_types::ip::Subnet;
    use net_types::SpecifiedAddr;
    use netstack3_base::testutil::{FakeDeviceId, MultipleDevicesId, TestIpExt};
    use test_case::test_case;

    use super::*;

    #[ip_test(I)]
    #[test_case(None, None => true)]
    #[test_case(None, Some(MultipleDevicesId::A) => true)]
    #[test_case(Some("A"), None => false)]
    #[test_case(Some("A"), Some(MultipleDevicesId::A) => true)]
    #[test_case(Some("A"), Some(MultipleDevicesId::B) => false)]
    fn rule_matcher_matches_device_name<I: TestIpExt>(
        device_name: Option<&str>,
        bound_device: Option<MultipleDevicesId>,
    ) -> bool {
        let matcher = RuleMatcher::<I> {
            source_address_matcher: None,
            traffic_origin_matcher: Some(TrafficOriginMatcher::Local {
                bound_device_matcher: device_name.map(|name| DeviceNameMatcher(name.into())),
            }),
        };
        let input = RuleInput {
            packet_origin: PacketOrigin::Local {
                bound_address: None,
                bound_device: bound_device.as_ref(),
            },
        };
        matcher.matches(&input)
    }

    #[ip_test(I)]
    #[test_case(None, None => true)]
    #[test_case(None, Some(I::LOOPBACK_ADDRESS) => true)]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.subnet),
        None => false)]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.subnet),
        Some(<I as TestIpExt>::TEST_ADDRS.local_ip) => true)]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.subnet),
        Some(<I as TestIpExt>::get_other_remote_ip_address(1)) => false)]
    fn rule_matcher_matches_local_addr<I: TestIpExt>(
        source_address_subnet: Option<Subnet<I::Addr>>,
        bound_address: Option<SpecifiedAddr<I::Addr>>,
    ) -> bool {
        let matcher = RuleMatcher::<I> {
            source_address_matcher: source_address_subnet.map(SubnetMatcher),
            traffic_origin_matcher: None,
        };
        let input = RuleInput::<'static, _, FakeDeviceId> {
            packet_origin: PacketOrigin::Local { bound_address, bound_device: None },
        };
        matcher.matches(&input)
    }

    #[ip_test(I)]
    #[test_case(None, PacketOrigin::Local {
         bound_address: None,
         bound_device: None
    } => true)]
    #[test_case(None, PacketOrigin::NonLocal {
        source_address: <I as TestIpExt>::TEST_ADDRS.remote_ip,
        incoming_device: &FakeDeviceId
    } => true)]
    #[test_case(Some(TrafficOriginMatcher::Local {
        bound_device_matcher: None
    }), PacketOrigin::Local {
        bound_address: None,
        bound_device: None
    } => true)]
    #[test_case(Some(TrafficOriginMatcher::NonLocal),
        PacketOrigin::NonLocal {
            source_address: <I as TestIpExt>::TEST_ADDRS.remote_ip,
            incoming_device: &FakeDeviceId
        } => true)]
    #[test_case(Some(TrafficOriginMatcher::Local { bound_device_matcher: None }),
        PacketOrigin::NonLocal {
            source_address: <I as TestIpExt>::TEST_ADDRS.remote_ip,
            incoming_device: &FakeDeviceId
        }  => false)]
    #[test_case(Some(TrafficOriginMatcher::NonLocal),
        PacketOrigin::Local {
            bound_address: None,
            bound_device: None
        } => false)]
    fn rule_matcher_matches_locally_generated<I: TestIpExt>(
        traffic_origin_matcher: Option<TrafficOriginMatcher>,
        packet_origin: PacketOrigin<I, &'static FakeDeviceId>,
    ) -> bool {
        let matcher = RuleMatcher::<I> { source_address_matcher: None, traffic_origin_matcher };
        let input = RuleInput::<'static, _, FakeDeviceId> { packet_origin };
        matcher.matches(&input)
    }

    #[ip_test(I)]
    #[test_case::test_matrix(
            [
                None,
                Some(<I as TestIpExt>::TEST_ADDRS.local_ip),
                Some(<I as TestIpExt>::get_other_remote_ip_address(1))
            ],
            [
                None,
                Some(&MultipleDevicesId::A),
                Some(&MultipleDevicesId::B),
                Some(&MultipleDevicesId::C),
            ],
            [true, false]
        )]
    fn rule_matcher_matches_multiple_conditions<I: TestIpExt>(
        ip: Option<SpecifiedAddr<I::Addr>>,
        device: Option<&'static MultipleDevicesId>,
        locally_generated: bool,
    ) {
        let matcher = RuleMatcher::<I> {
            source_address_matcher: Some(SubnetMatcher(I::TEST_ADDRS.subnet)),
            traffic_origin_matcher: Some(TrafficOriginMatcher::Local {
                bound_device_matcher: Some(DeviceNameMatcher("A".into())),
            }),
        };

        let packet_origin = if locally_generated {
            PacketOrigin::Local { bound_address: ip, bound_device: device }
        } else {
            let (Some(source_address), Some(incoming_device)) = (ip, device) else {
                return;
            };
            PacketOrigin::NonLocal { source_address, incoming_device }
        };

        let input = RuleInput { packet_origin };

        if ip == Some(I::TEST_ADDRS.local_ip)
            && (device == Some(&MultipleDevicesId::A))
            && locally_generated
        {
            assert!(matcher.matches(&input))
        } else {
            assert!(!matcher.matches(&input))
        }
    }
}
