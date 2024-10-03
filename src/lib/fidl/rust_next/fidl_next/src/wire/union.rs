// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use munge::munge;

use crate::{decode, encode, u64_le, Decode, Decoder, Encode, Encoder, Slot, WireEnvelope};

/// A raw FIDL union
#[repr(C)]
pub struct RawWireUnion<'buf> {
    ordinal: u64_le,
    envelope: WireEnvelope<'buf>,
}

impl<'buf> RawWireUnion<'buf> {
    /// Encodes that a union is absent in a slot.
    pub fn encode_absent(slot: Slot<'_, Self>) {
        munge!(let Self { mut ordinal, envelope } = slot);

        *ordinal = u64_le::from_native(0);
        WireEnvelope::encode_zero(envelope);
    }

    /// Encodes a value and ordinal in a slot.
    pub fn encode_as<E: Encoder + ?Sized, T: Encode<E>>(
        value: &mut T,
        ord: u64,
        encoder: &mut E,
        slot: Slot<'_, Self>,
    ) -> Result<(), encode::EncodeError> {
        munge!(let Self { mut ordinal, envelope } = slot);

        *ordinal = u64_le::from_native(ord);
        WireEnvelope::encode_value(value, encoder, envelope)
    }

    /// Returns the ordinal of the encoded value.
    pub fn encoded_ordinal(slot: Slot<'_, Self>) -> u64 {
        munge!(let Self { ordinal, envelope: _ } = slot);
        ordinal.to_native()
    }

    /// Decodes an absent union from a slot.
    pub fn decode_absent(slot: Slot<'_, Self>) -> Result<(), decode::DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        if !WireEnvelope::is_encoded_zero(envelope) {
            return Err(decode::DecodeError::InvalidUnionEnvelope);
        }
        Ok(())
    }

    /// Decodes an unknown value from a union.
    ///
    /// The handles owned by the unknown value are discarded.
    pub fn decode_unknown<D: Decoder<'buf> + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), decode::DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        WireEnvelope::decode_unknown(envelope, decoder)
    }

    /// Decodes the typed value in a union.
    pub fn decode_as<D: Decoder<'buf> + ?Sized, T: Decode<D>>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), decode::DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        WireEnvelope::decode_as::<D, T>(envelope, decoder)
    }

    /// The null (absent) union.
    pub fn null() -> Self {
        Self { ordinal: u64_le::from_native(0), envelope: WireEnvelope::zero() }
    }

    /// Returns whether the union contains a value.
    pub fn is_some(&self) -> bool {
        self.ordinal.to_native() != 0
    }

    /// Returns whether the union is empty.
    pub fn is_none(&self) -> bool {
        !self.is_some()
    }

    /// Returns the ordinal of the union.
    pub fn ordinal(&self) -> u64 {
        self.ordinal.to_native()
    }

    /// Gets a reference to the envelope underlying the union.
    pub fn get(&self) -> &WireEnvelope<'buf> {
        &self.envelope
    }

    /// Gets a mutable reference to the envelope underlying the union.
    pub fn get_mut(&mut self) -> &mut WireEnvelope<'buf> {
        &mut self.envelope
    }
}
