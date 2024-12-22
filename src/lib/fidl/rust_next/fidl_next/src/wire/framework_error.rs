// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::hint::unreachable_unchecked;

use munge::munge;
use rend::i32_le;

use crate::{Decode, DecodeError, Encodable, Encode, EncodeError, FrameworkError, Slot, TakeFrom};

/// An internal framework error.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct WireFrameworkError {
    inner: i32_le,
}

impl fmt::Debug for WireFrameworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        FrameworkError::from(*self).fmt(f)
    }
}

impl From<WireFrameworkError> for FrameworkError {
    fn from(value: WireFrameworkError) -> Self {
        match value.inner.to_native() {
            -2 => Self::UnknownMethod,
            _ => unsafe { unreachable_unchecked() },
        }
    }
}

unsafe impl<D> Decode<D> for WireFrameworkError {
    fn decode(slot: Slot<'_, Self>, _: &mut D) -> Result<(), DecodeError> {
        munge!(let Self { inner } = slot);
        match inner.to_native() {
            -2 => Ok(()),
            code => Err(DecodeError::InvalidFrameworkError(code)),
        }
    }
}

impl Encodable for FrameworkError {
    type Encoded<'buf> = WireFrameworkError;
}

impl<E: ?Sized> Encode<E> for FrameworkError {
    fn encode(&mut self, _: &mut E, slot: Slot<'_, Self::Encoded<'_>>) -> Result<(), EncodeError> {
        munge!(let WireFrameworkError { mut inner } = slot);
        inner.write(i32_le::from_native(match self {
            Self::UnknownMethod => -2,
        }));

        Ok(())
    }
}

impl TakeFrom<WireFrameworkError> for FrameworkError {
    fn take_from(from: &mut WireFrameworkError) -> Self {
        Self::from(*from)
    }
}
