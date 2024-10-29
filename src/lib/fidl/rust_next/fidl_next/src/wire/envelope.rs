// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::ptr::addr_of_mut;

use munge::munge;

use crate::{
    decode, encode, u16_le, u32_le, Chunk, Decode, Decoder, DecoderExt as _, Encode, Encoder,
    EncoderExt as _, Slot, CHUNK_SIZE,
};

#[derive(Clone, Copy)]
#[repr(C)]
struct Encoded {
    maybe_num_bytes: u32_le,
    num_handles: u16_le,
    flags: u16_le,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct DecodedOutOfLine<'buf> {
    ptr: *mut (),
    _phantom: PhantomData<&'buf mut [Chunk]>,
}

/// A FIDL envelope
#[repr(C, align(8))]
pub union WireEnvelope<'buf> {
    zero: [u8; 8],
    encoded: Encoded,
    decoded_inline: [u8; 4],
    decoded_out_of_line: DecodedOutOfLine<'buf>,
}

impl<'buf> WireEnvelope<'buf> {
    const IS_INLINE_BIT: u16 = 1;

    /// Encodes a zero envelope into a slot.
    pub fn encode_zero(slot: Slot<'_, Self>) {
        munge!(let Self { mut zero } = slot);
        *zero = [0; 8];
    }

    /// Encodes a value into an envelope with an encoder.
    pub fn encode_value<E: Encoder + ?Sized, T: Encode<E>>(
        value: &mut T,
        encoder: &mut E,
        slot: Slot<'_, Self>,
    ) -> Result<(), encode::EncodeError> {
        munge! {
            let Self {
                encoded: Encoded {
                    mut maybe_num_bytes,
                    mut num_handles,
                    mut flags,
                },
            } = slot;
        }

        let handles_before = encoder.__internal_handle_count();

        if size_of::<T::Encoded<'_>>() <= 4 {
            let slot = unsafe { Slot::new_unchecked(maybe_num_bytes.as_mut_ptr().cast()) };
            value.encode(encoder, slot)?;

            *flags = u16_le::from_native(Self::IS_INLINE_BIT);
        } else {
            let bytes_before = encoder.bytes_written();

            encoder.encode_next(value)?;

            *maybe_num_bytes =
                u32_le::from_native((encoder.bytes_written() - bytes_before).try_into().unwrap());
        }

        *num_handles = u16_le::from_native(
            (encoder.__internal_handle_count() - handles_before).try_into().unwrap(),
        );

        Ok(())
    }

    /// Returns the zero envelope.
    pub fn zero() -> Self {
        Self { zero: [0; 8] }
    }

    /// Returns whether a envelope slot is encoded as zero.
    pub fn is_encoded_zero(slot: Slot<'_, Self>) -> bool {
        munge!(let Self { zero } = slot);
        *zero == [0; 8]
    }

    /// Returns whether an envelope is zero.
    pub fn is_zero(&self) -> bool {
        unsafe { self.zero == [0; 8] }
    }

    fn out_of_line_chunks(
        maybe_num_bytes: Slot<'_, u32_le>,
        flags: Slot<'_, u16_le>,
    ) -> Result<Option<usize>, decode::DecodeError> {
        if flags.to_native() & Self::IS_INLINE_BIT == 0 {
            let num_bytes = maybe_num_bytes.to_native();
            if num_bytes as usize % CHUNK_SIZE != 0 {
                return Err(decode::DecodeError::InvalidEnvelopeSize(num_bytes));
            }
            if num_bytes <= 4 {
                return Err(decode::DecodeError::OutOfLineValueTooSmall(num_bytes));
            }
            Ok(Some(num_bytes as usize / CHUNK_SIZE))
        } else {
            Ok(None)
        }
    }

    /// Decodes and discards an unknown value in an envelope.
    pub fn decode_unknown<D: Decoder<'buf> + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), decode::DecodeError> {
        munge! {
            let Self {
                encoded: Encoded {
                    maybe_num_bytes,
                    num_handles,
                    flags,
                },
            } = slot;
        }

        if let Some(count) = Self::out_of_line_chunks(maybe_num_bytes, flags)? {
            decoder.take_chunks(count)?;
        }

        decoder.__internal_take_handles(num_handles.to_native() as usize)?;

        Ok(())
    }

    /// Decodes a value of a known type from an envelope.
    pub fn decode_as<D: Decoder<'buf> + ?Sized, T: Decode<D>>(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), decode::DecodeError> {
        munge! {
            let Self {
                encoded: Encoded {
                    mut maybe_num_bytes,
                    num_handles,
                    flags,
                },
             } = slot.as_mut();
        }

        let handles_before = decoder.__internal_handles_remaining();
        let num_handles = num_handles.to_native() as usize;

        let out_of_line_chunks = Self::out_of_line_chunks(maybe_num_bytes.as_mut(), flags)?;
        if let Some(_count) = out_of_line_chunks {
            // Decode out-of-line value
            // TODO: set cap on decoder to make sure that the envelope doesn't decode more bytes
            // than it claims that it will
            let mut value_slot = decoder.take_slot::<T>()?;
            let value_ptr = value_slot.as_mut_ptr();
            T::decode(value_slot, decoder)?;

            munge! {
                let Self {
                    decoded_out_of_line: DecodedOutOfLine {
                        mut ptr,
                        _phantom,
                    },
                } = slot;
            }
            // SAFETY: Identical to `ptr.write(value_ptr.cast())`, but raw
            // pointers don't currently implement `IntoBytes`.
            unsafe { ptr.as_mut_ptr().write(value_ptr.cast()) };
        } else {
            // Decode inline value
            if size_of::<T>() > 4 {
                return Err(decode::DecodeError::InlineValueTooBig(size_of::<T>()));
            }
            munge!(let Self { mut decoded_inline } = slot);
            let mut slot = unsafe { Slot::<T>::new_unchecked(decoded_inline.as_mut_ptr().cast()) };
            T::decode(slot.as_mut(), decoder)?;
        }

        let handles_consumed = handles_before - decoder.__internal_handles_remaining();
        if handles_consumed != num_handles {
            return Err(decode::DecodeError::IncorrectNumberOfHandlesConsumed {
                expected: num_handles,
                actual: handles_consumed,
            });
        }

        Ok(())
    }

    unsafe fn as_ptr<T>(this: *mut Self) -> *mut T {
        if size_of::<T>() <= 4 {
            let inline = unsafe { addr_of_mut!((*this).decoded_inline) };
            inline.cast()
        } else {
            unsafe { (*this).decoded_out_of_line.ptr.cast() }
        }
    }

    /// Returns a reference to the contained `T`.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded as a `T`.
    pub unsafe fn deref_unchecked<T>(&self) -> &T {
        let ptr = unsafe { Self::as_ptr::<T>((self as *const Self).cast_mut()).cast_const() };
        unsafe { &*ptr }
    }

    /// Returns a mutable reference to the contained `T`.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded as a `T`.
    pub unsafe fn deref_mut_unchecked<T>(&mut self) -> &mut T {
        let ptr = unsafe { Self::as_ptr::<T>(self as *mut Self) };
        unsafe { &mut *ptr }
    }

    /// Takes the contained `T`, leaving an empty envelope.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded as a `T`.
    pub unsafe fn take_unchecked<T>(&mut self) -> T {
        let ptr = unsafe { Self::as_ptr::<T>(self as *mut Self) };
        let result = unsafe { ptr.read() };
        *self = Self { zero: [0; 8] };
        result
    }
}
