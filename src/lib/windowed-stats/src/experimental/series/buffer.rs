// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ring buffers and compression.

use num::Unsigned;
use std::io::{self, Write};
use tracing::warn;

use crate::experimental::ring_buffer::{
    Simple8bRleRingBuffer, UncompressedRingBuffer, ZigzagSimple8bRleRingBuffer,
};
use crate::experimental::series::interpolation::Interpolation;
use crate::experimental::series::statistic::Aggregation;
use crate::experimental::series::SamplingInterval;

/// A type that can construct a [`RingBuffer`] associated with an aggregation type and
/// interpolation.
///
/// [`RingBuffer`]: crate::experimental::series::buffer::RingBuffer
pub trait BufferStrategy<A, P>
where
    P: Interpolation,
{
    type Buffer: Clone + RingBuffer<A>;

    /// Constructs a ring buffer with the given fixed capacity.
    fn buffer(interval: &SamplingInterval) -> Self::Buffer {
        Self::Buffer::with_capacity(interval.capacity() as usize)
    }
    /// Get the descriptive type of the buffer.
    fn buffer_type() -> RingBufferType {
        Self::Buffer::buffer_type()
    }
}

pub type Buffer<F, P> = <F as BufferStrategy<Aggregation<F>, P>>::Buffer;

/// A fixed-capacity circular ring buffer.
pub trait RingBuffer<A> {
    fn with_capacity(capacity: usize) -> Self
    where
        Self: Sized;

    fn buffer_type() -> RingBufferType;

    fn push(&mut self, item: A) {
        let _ = item;
    }

    fn serialize(&self, write: impl Write) -> io::Result<()> {
        let _ = write;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum RingBufferType {
    Uncompressed(UncompressedSubtype),
    Simple8bRle(Simple8bRleSubtype),
    DeltaEncodedSimple8bRle(Simple8bRleSubtype),
}

impl RingBufferType {
    pub fn type_descriptor(&self) -> u8 {
        match self {
            Self::Uncompressed(_) => 0,
            Self::Simple8bRle(_) => 1,
            Self::DeltaEncodedSimple8bRle(_) => 2,
        }
    }

    pub fn subtype_descriptor(&self) -> u8 {
        match self {
            Self::Uncompressed(subtype) => match subtype {
                UncompressedSubtype::F32 => 0,
            },
            Self::Simple8bRle(subtype) | Self::DeltaEncodedSimple8bRle(subtype) => match subtype {
                Simple8bRleSubtype::Unsigned => 0,
                Simple8bRleSubtype::SignedZigzagEncoded => 1,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum UncompressedSubtype {
    F32,
}

#[derive(Clone, Debug)]
pub enum Simple8bRleSubtype {
    Unsigned,
    SignedZigzagEncoded,
}

#[derive(Clone, Debug)]
pub struct Uncompressed<A>(UncompressedRingBuffer<A>);

impl RingBuffer<f32> for Uncompressed<f32> {
    fn with_capacity(capacity: usize) -> Self {
        let ring_buffer = UncompressedRingBuffer::with_min_samples(capacity);
        Uncompressed(ring_buffer)
    }

    fn buffer_type() -> RingBufferType {
        RingBufferType::Uncompressed(UncompressedSubtype::F32)
    }

    fn push(&mut self, item: f32) {
        self.0.push(item);
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

#[derive(Clone, Debug)]
pub struct Simple8bRle(Simple8bRleRingBuffer);

impl<A> RingBuffer<A> for Simple8bRle
where
    A: Into<u64> + Unsigned,
{
    fn with_capacity(capacity: usize) -> Self {
        let ring_buffer = Simple8bRleRingBuffer::with_min_samples(capacity);
        Simple8bRle(ring_buffer)
    }

    fn buffer_type() -> RingBufferType {
        RingBufferType::Simple8bRle(Simple8bRleSubtype::Unsigned)
    }

    fn push(&mut self, item: A) {
        self.0.push(item.into());
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

#[derive(Clone, Debug)]
pub struct ZigzagSimple8bRle(ZigzagSimple8bRleRingBuffer);

impl<A> RingBuffer<A> for ZigzagSimple8bRle
where
    A: Into<i64>,
{
    fn with_capacity(capacity: usize) -> Self {
        let ring_buffer = ZigzagSimple8bRleRingBuffer::with_min_samples(capacity);
        ZigzagSimple8bRle(ring_buffer)
    }

    fn buffer_type() -> RingBufferType {
        RingBufferType::Simple8bRle(Simple8bRleSubtype::SignedZigzagEncoded)
    }

    fn push(&mut self, item: A) {
        self.0.push(item.into());
    }

    fn serialize(&self, mut write: impl Write) -> io::Result<()> {
        self.0.serialize(&mut write)
    }
}

// TODO(https://fxbug.dev/352614791): Implement DeltaSimple8bRle ring buffer
/// A ring buffer that stores unsigned integer items using delta, Simple8B, and run length encoding.
#[derive(Clone, Debug)]
pub struct DeltaSimple8bRle;

impl<A> RingBuffer<A> for DeltaSimple8bRle
where
    A: Into<u64> + Unsigned,
{
    fn with_capacity(capacity: usize) -> Self {
        warn!("DeltaSimple8bRle ring buffer is unimplemented. No data will be stored.");
        let _ = capacity;
        DeltaSimple8bRle
    }

    fn buffer_type() -> RingBufferType {
        RingBufferType::DeltaEncodedSimple8bRle(Simple8bRleSubtype::Unsigned)
    }
}

// TODO(https://fxbug.dev/352614791): Implement DeltaZigZagSimple8bRle ring buffer
/// A ring buffer that stores integer items using delta, zig-zag, Simple8B, and run length
/// encoding.
#[derive(Clone, Debug)]
pub struct DeltaZigZagSimple8bRle;

impl RingBuffer<i64> for DeltaZigZagSimple8bRle {
    fn with_capacity(capacity: usize) -> Self {
        warn!("DeltaZigZagSimple8bRle ring buffer is unimplemented. No data will be stored.");
        let _ = capacity;
        DeltaZigZagSimple8bRle
    }

    fn buffer_type() -> RingBufferType {
        RingBufferType::DeltaEncodedSimple8bRle(Simple8bRleSubtype::SignedZigzagEncoded)
    }
}

impl RingBuffer<u64> for DeltaZigZagSimple8bRle {
    fn with_capacity(capacity: usize) -> Self {
        warn!("DeltaZigZagSimple8bRle ring buffer is unimplemented. No data will be stored.");
        let _ = capacity;
        DeltaZigZagSimple8bRle
    }

    fn buffer_type() -> RingBufferType {
        RingBufferType::DeltaEncodedSimple8bRle(Simple8bRleSubtype::SignedZigzagEncoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncompressed_buffer() {
        let mut buffer = <Uncompressed<f32> as RingBuffer<f32>>::with_capacity(2);
        buffer.push(22f32);
        let mut data = vec![];
        let result = RingBuffer::<f32>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn simple8b_rle_buffer() {
        let mut buffer = <Simple8bRle as RingBuffer<u64>>::with_capacity(2);
        buffer.push(22u64);
        let mut data = vec![];
        let result = RingBuffer::<u64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }

    #[test]
    fn zigzag_simple8b_rle_buffer() {
        let mut buffer = <ZigzagSimple8bRle as RingBuffer<i64>>::with_capacity(2);
        buffer.push(22i64);
        let mut data = vec![];
        let result = RingBuffer::<i64>::serialize(&buffer, &mut data);
        assert!(result.is_ok());
        assert!(!data.is_empty());
    }
}
