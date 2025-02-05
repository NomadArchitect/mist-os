// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod transport;

use crate::{Chunk, Decode, DecoderExt as _, Encode, EncoderExt as _, Owned};

pub fn assert_encoded<T: Encode<Vec<Chunk>>>(mut value: T, chunks: &[Chunk]) {
    let mut encoded_chunks = Vec::new();
    encoded_chunks.encode_next(&mut value).unwrap();
    assert_eq!(encoded_chunks, chunks, "encoded chunks did not match");
}

pub fn assert_decoded<'buf, T: Decode<&'buf mut [Chunk]>>(
    mut chunks: &'buf mut [Chunk],
    f: impl FnOnce(Owned<'buf, T>),
) {
    let value = chunks.decode_next::<T>().expect("failed to decode");
    f(value)
}
