// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::str::from_utf8;

use munge::munge;

use super::WireString;
use crate::encode::{self, EncodeOption};
use crate::wire::WireOptionalVector;
use crate::{decode, Decode, Slot, TakeFrom, WireVector};

/// An optional FIDL string
#[derive(Default)]
#[repr(transparent)]
pub struct WireOptionalString<'buf> {
    vec: WireOptionalVector<'buf, u8>,
}

impl<'buf> WireOptionalString<'buf> {
    /// Encodes that a string is present in a slot.
    pub fn encode_present(slot: Slot<'_, Self>, len: u64) {
        munge!(let Self { vec } = slot);
        WireOptionalVector::encode_present(vec, len);
    }

    /// Encodes that a string is absent in a slot.
    pub fn encode_absent(slot: Slot<'_, Self>) {
        munge!(let Self { vec } = slot);
        WireOptionalVector::encode_absent(vec);
    }

    /// Returns whether the optional string is present.
    pub fn is_some(&self) -> bool {
        self.vec.is_some()
    }

    /// Returns whether the optional string is absent.
    pub fn is_none(&self) -> bool {
        self.vec.is_none()
    }

    /// Takes the string out of the option, if any.
    pub fn take(&mut self) -> Option<WireString<'buf>> {
        self.vec.take().map(|vec| unsafe { WireString::new_unchecked(vec) })
    }

    /// Returns a reference to the underlying string, if any.
    pub fn as_ref(&self) -> Option<&WireString<'buf>> {
        self.vec.as_ref().map(|vec| unsafe { &*(vec as *const WireVector<'buf, u8>).cast() })
    }

    /// Returns a mutable reference to the underlying string, if any.
    pub fn as_mut(&mut self) -> Option<&mut WireString<'buf>> {
        self.vec.as_mut().map(|vec| unsafe { &mut *(vec as *mut WireVector<'buf, u8>).cast() })
    }
}

impl fmt::Debug for WireOptionalString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

unsafe impl<'buf> Decode<'buf> for WireOptionalString<'buf> {
    fn decode(
        slot: Slot<'_, Self>,
        decoder: &mut decode::Decoder<'buf>,
    ) -> Result<(), decode::Error> {
        munge!(let Self { mut vec } = slot);

        WireOptionalVector::decode(vec.as_mut(), decoder)?;
        let vec = unsafe { vec.deref_unchecked() };
        if let Some(bytes) = vec.as_ref() {
            from_utf8(bytes)?;
        }

        Ok(())
    }
}

impl EncodeOption for String {
    type EncodedOption<'buf> = WireOptionalString<'buf>;

    fn encode_option(
        this: Option<&mut Self>,
        encoder: &mut encode::Encoder,
        slot: Slot<'_, Self::EncodedOption<'_>>,
    ) -> Result<(), encode::Error> {
        if let Some(string) = this {
            encoder.write_bytes(string.as_bytes());
            WireOptionalString::encode_present(slot, string.len() as u64);
        } else {
            WireOptionalString::encode_absent(slot);
        }

        Ok(())
    }
}

impl TakeFrom<WireOptionalString<'_>> for Option<String> {
    fn take_from(from: &mut WireOptionalString<'_>) -> Self {
        from.as_mut().map(String::take_from)
    }
}
