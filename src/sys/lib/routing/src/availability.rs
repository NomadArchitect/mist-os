// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::AvailabilityRoutingError;
use cm_rust::{Availability, ExposeDeclCommon, ExposeSource, OfferDeclCommon, OfferSource};
use fidl_fuchsia_component_sandbox as fsandbox;
use moniker::ExtendedMoniker;
use sandbox::{Capability, Data, Dict, DictKey};

const AVAILABILITY_KEY: &'static str = "availability";

pub fn advance_with_offer(
    moniker: &ExtendedMoniker,
    current: Availability,
    offer: &impl OfferDeclCommon,
) -> Result<Availability, AvailabilityRoutingError> {
    let result = advance(moniker, current, *offer.availability());
    if offer.source() == &OfferSource::Void
        && result
            == Err(AvailabilityRoutingError::TargetHasStrongerAvailability {
                moniker: moniker.clone(),
            })
    {
        return Err(AvailabilityRoutingError::OfferFromVoidToRequiredTarget {
            moniker: moniker.clone(),
        });
    }
    result
}

pub fn advance_with_expose(
    moniker: &ExtendedMoniker,
    current: Availability,
    expose: &impl ExposeDeclCommon,
) -> Result<Availability, AvailabilityRoutingError> {
    let result = advance(moniker, current, *expose.availability());
    if expose.source() == &ExposeSource::Void
        && result
            == Err(AvailabilityRoutingError::TargetHasStrongerAvailability {
                moniker: moniker.clone(),
            })
    {
        return Err(AvailabilityRoutingError::ExposeFromVoidToRequiredTarget {
            moniker: moniker.clone(),
        });
    }
    result
}

/// A type which has accessors for route request `Availability` metadata.
pub trait AvailabilityMetadata {
    /// Infallibly assigns `value` to `self`.
    fn set_availability(&self, value: Availability);

    /// Retrieves the `Availability` metadata from `self`, if present.
    fn get_availability(&self) -> Option<Availability>;
}

impl AvailabilityMetadata for Dict {
    fn set_availability(&self, value: Availability) {
        let key = DictKey::new(AVAILABILITY_KEY).expect("dict key creation failed unexpectedly");
        match self.insert(key, Capability::Data(Data::String(value.to_string()))) {
            // When an entry already exists for a key in a Dict, insert() will
            // still replace that entry with the new value, even though it
            // returns an ItemAlreadyExists error. As a result, we can treat
            // ItemAlreadyExists as a success case.
            Ok(()) | Err(fsandbox::CapabilityStoreError::ItemAlreadyExists) => (),
            // Dict::insert() only returns `CapabilityStoreError::ItemAlreadyExists` variant
            Err(e) => panic!("unexpected error variant returned from Dict::insert(): {e:?}"),
        }
    }

    fn get_availability(&self) -> Option<Availability> {
        let key = DictKey::new(AVAILABILITY_KEY).expect("dict key creation failed unexpectedly");
        let capability = self.get(&key).ok()??;
        match capability {
            Capability::Data(Data::String(availability)) => match availability.as_str() {
                "Optional" => Some(Availability::Optional),
                "Required" => Some(Availability::Required),
                "SameAsTarget" => Some(Availability::SameAsTarget),
                "Transitional" => Some(Availability::Transitional),
                _ => None,
            },
            _ => None,
        }
    }
}

impl crate::legacy_router::OfferVisitor for Availability {
    fn visit(
        &mut self,
        moniker: &ExtendedMoniker,
        offer: &cm_rust::OfferDecl,
    ) -> Result<(), crate::RoutingError> {
        *self = advance_with_offer(moniker, *self, offer)?;
        Ok(())
    }
}

impl crate::legacy_router::ExposeVisitor for Availability {
    fn visit(
        &mut self,
        moniker: &ExtendedMoniker,
        expose: &cm_rust::ExposeDecl,
    ) -> Result<(), crate::RoutingError> {
        *self = advance_with_expose(moniker, *self, expose)?;
        Ok(())
    }
}

impl crate::legacy_router::CapabilityVisitor for Availability {
    fn visit(
        &mut self,
        _: &ExtendedMoniker,
        _: &cm_rust::CapabilityDecl,
    ) -> Result<(), crate::RoutingError> {
        Ok(())
    }
}

pub fn advance(
    moniker: &ExtendedMoniker,
    current: Availability,
    next_availability: Availability,
) -> Result<Availability, AvailabilityRoutingError> {
    let next = availability::advance(moniker, current, next_availability)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_rust::{
        DependencyType, ExposeDecl, ExposeProtocolDecl, ExposeTarget, OfferDecl, OfferProtocolDecl,
    };
    use cm_rust_testing::*;
    use test_case::test_case;

    fn new_offer(availability: Availability) -> OfferDecl {
        OfferDecl::Protocol(OfferProtocolDecl {
            source: OfferSource::Parent,
            source_name: "fuchsia.examples.Echo".parse().unwrap(),
            source_dictionary: Default::default(),
            target: offer_target_static_child("echo"),
            target_name: "fuchsia.examples.Echo".parse().unwrap(),
            dependency_type: DependencyType::Weak,
            availability,
        })
    }

    fn new_void_offer() -> OfferDecl {
        OfferDecl::Protocol(OfferProtocolDecl {
            source: OfferSource::Void,
            source_name: "fuchsia.examples.Echo".parse().unwrap(),
            source_dictionary: Default::default(),
            target: offer_target_static_child("echo"),
            target_name: "fuchsia.examples.Echo".parse().unwrap(),
            dependency_type: DependencyType::Weak,
            availability: Availability::Optional,
        })
    }

    #[test_case(Availability::Optional, new_offer(Availability::Optional), Ok(()))]
    #[test_case(Availability::Optional, new_offer(Availability::Required), Ok(()))]
    #[test_case(Availability::Optional, new_offer(Availability::SameAsTarget), Ok(()))]
    #[test_case(
        Availability::Optional,
        new_offer(Availability::Transitional),
        Err(AvailabilityRoutingError::TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Optional, new_void_offer(), Ok(()))]
    #[test_case(
        Availability::Required,
        new_offer(Availability::Optional),
        Err(AvailabilityRoutingError::TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Required, new_offer(Availability::Required), Ok(()))]
    #[test_case(Availability::Required, new_offer(Availability::SameAsTarget), Ok(()))]
    #[test_case(
        Availability::Required,
        new_offer(Availability::Transitional),
        Err(AvailabilityRoutingError::TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(
        Availability::Required,
        new_void_offer(),
        Err(AvailabilityRoutingError::OfferFromVoidToRequiredTarget { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Transitional, new_offer(Availability::Optional), Ok(()))]
    #[test_case(Availability::Transitional, new_offer(Availability::Required), Ok(()))]
    #[test_case(Availability::Transitional, new_offer(Availability::SameAsTarget), Ok(()))]
    #[test_case(Availability::Transitional, new_offer(Availability::Transitional), Ok(()))]
    #[test_case(Availability::Transitional, new_void_offer(), Ok(()))]
    fn offer_tests(
        availability: Availability,
        offer: OfferDecl,
        expected: Result<(), AvailabilityRoutingError>,
    ) {
        let actual = advance_with_offer(&ExtendedMoniker::ComponentManager, availability, &offer)
            .map(|_| ());
        assert_eq!(actual, expected);
    }

    fn new_expose(availability: Availability) -> ExposeDecl {
        ExposeDecl::Protocol(ExposeProtocolDecl {
            source: ExposeSource::Self_,
            source_name: "fuchsia.examples.Echo".parse().unwrap(),
            source_dictionary: Default::default(),
            target: ExposeTarget::Parent,
            target_name: "fuchsia.examples.Echo".parse().unwrap(),
            availability,
        })
    }

    fn new_void_expose() -> ExposeDecl {
        ExposeDecl::Protocol(ExposeProtocolDecl {
            source: ExposeSource::Void,
            source_name: "fuchsia.examples.Echo".parse().unwrap(),
            source_dictionary: Default::default(),
            target: ExposeTarget::Parent,
            target_name: "fuchsia.examples.Echo".parse().unwrap(),
            availability: Availability::Optional,
        })
    }

    #[test_case(Availability::Optional, new_expose(Availability::Optional), Ok(()))]
    #[test_case(Availability::Optional, new_expose(Availability::Required), Ok(()))]
    #[test_case(Availability::Optional, new_expose(Availability::SameAsTarget), Ok(()))]
    #[test_case(
        Availability::Optional,
        new_expose(Availability::Transitional),
        Err(AvailabilityRoutingError::TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(
        Availability::Optional,
        new_void_expose(),
        Ok(())
    )]
    #[test_case(
        Availability::Required,
        new_expose(Availability::Optional),
        Err(AvailabilityRoutingError::TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Required, new_expose(Availability::Required), Ok(()))]
    #[test_case(Availability::Required, new_expose(Availability::SameAsTarget), Ok(()))]
    #[test_case(
        Availability::Required,
        new_expose(Availability::Transitional),
        Err(AvailabilityRoutingError::TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(
        Availability::Required,
        new_void_expose(),
        Err(AvailabilityRoutingError::ExposeFromVoidToRequiredTarget { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Transitional, new_expose(Availability::Optional), Ok(()))]
    #[test_case(Availability::Transitional, new_expose(Availability::Required), Ok(()))]
    #[test_case(Availability::Transitional, new_expose(Availability::SameAsTarget), Ok(()))]
    #[test_case(Availability::Transitional, new_expose(Availability::Transitional), Ok(()))]
    #[test_case(
        Availability::Transitional,
        new_void_expose(),
        Ok(())
    )]
    fn expose_tests(
        availability: Availability,
        expose: ExposeDecl,
        expected: Result<(), AvailabilityRoutingError>,
    ) {
        let actual = advance_with_expose(&ExtendedMoniker::ComponentManager, availability, &expose)
            .map(|_| ());
        assert_eq!(actual, expected);
    }
}
