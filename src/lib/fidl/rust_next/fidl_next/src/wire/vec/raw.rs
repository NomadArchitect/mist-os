// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ptr::slice_from_raw_parts_mut;

use munge::munge;

use crate::{u64_le, Decode, DecodeError, Decoder, DecoderExt, Owned, Slot, WirePointer};

#[repr(C)]
pub struct RawWireVector<'buf, T> {
    len: u64_le,
    ptr: WirePointer<'buf, T>,
}

impl<T> Drop for RawWireVector<'_, T> {
    fn drop(&mut self) {
        unsafe {
            self.as_slice_ptr().drop_in_place();
        }
    }
}

impl<T> RawWireVector<'_, T> {
    pub fn dangling() -> Self {
        Self { len: u64_le::from_native(0), ptr: WirePointer::dangling() }
    }

    pub fn null() -> Self {
        Self { len: u64_le::from_native(0), ptr: WirePointer::null() }
    }

    pub fn encode_present(slot: Slot<'_, Self>, len: u64) {
        munge!(let Self { len: mut encoded_len, ptr } = slot);
        *encoded_len = u64_le::from_native(len);
        WirePointer::encode_present(ptr);
    }

    pub fn encode_absent(slot: Slot<'_, Self>) {
        munge!(let Self { mut len, ptr } = slot);
        *len = u64_le::from_native(0);
        WirePointer::encode_absent(ptr);
    }

    pub fn len(&self) -> u64 {
        self.len.to_native()
    }

    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    pub fn as_slice_ptr(&self) -> *mut [T] {
        slice_from_raw_parts_mut(self.as_ptr(), self.len().try_into().unwrap())
    }
}

unsafe impl<'buf, D: Decoder<'buf> + ?Sized, T: Decode<D>> Decode<D> for RawWireVector<'buf, T> {
    fn decode(slot: Slot<'_, Self>, decoder: &mut D) -> Result<(), DecodeError> {
        munge!(let Self { len, mut ptr } = slot);

        let len = len.to_native();
        if WirePointer::is_encoded_present(ptr.as_mut())? {
            let slice = decoder.decode_next_slice::<T>(len as usize)?;
            let slice = unsafe { Owned::new_unchecked(slice.into_raw().cast::<T>()) };
            WirePointer::set_decoded(ptr, slice);
        }

        Ok(())
    }
}
