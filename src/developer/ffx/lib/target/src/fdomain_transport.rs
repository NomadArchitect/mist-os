// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::{AsyncBufRead, AsyncWrite};

/// Implements a transport for the FDomain client library on top of some async IO readers.
pub struct FDomainTransport {
    input: Pin<Box<dyn AsyncBufRead + Unpin + Send>>,
    output: Pin<Box<dyn AsyncWrite + Unpin + Send>>,
    write_progress: usize,
    in_buf: Vec<u8>,
}

impl FDomainTransport {
    /// construct a new `FDomainTransport`.
    pub fn new(
        input: Box<dyn AsyncBufRead + Unpin + Send>,
        output: Box<dyn AsyncWrite + Unpin + Send>,
    ) -> Self {
        FDomainTransport {
            input: Pin::new(input),
            output: Pin::new(output),
            write_progress: 0,
            in_buf: Vec::new(),
        }
    }
}

impl fdomain_client::FDomainTransport for FDomainTransport {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        ctx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        if self.write_progress < 4 {
            let size: u32 = msg
                .len()
                .try_into()
                .map_err(|_| std::io::Error::other("Message size exceeded u32 capacity"))?;
            let out_buf = size.to_le_bytes();

            while self.write_progress < 4 {
                let got = ready!(self.output.as_mut().poll_write(ctx, &out_buf))?;
                self.write_progress += got;
            }
        }

        while self.write_progress - 4 < msg.len() {
            let offset = self.write_progress - 4;
            let got = ready!(self.output.as_mut().poll_write(ctx, &msg[offset..]))?;
            self.write_progress += got;
        }

        self.write_progress = 0;
        Poll::Ready(Ok(()))
    }
}

impl futures::Stream for FDomainTransport {
    type Item = std::io::Result<Box<[u8]>>;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.in_buf.len() > 4 {
                let len: usize =
                    u32::from_le_bytes(self.in_buf[..4].try_into().unwrap()).try_into().unwrap();

                if self.in_buf.len() >= len + 4 {
                    let tail = self.in_buf.split_off(len + 4);
                    let mut got = std::mem::replace(&mut self.in_buf, tail);
                    got.drain(..4);
                    return Poll::Ready(Some(Ok(got.into())));
                }
            }

            let this = &mut *self;

            let buf = ready!(this.input.as_mut().poll_fill_buf(ctx))?;

            if buf.is_empty() {
                return Poll::Ready(None);
            }

            this.in_buf.extend_from_slice(buf);
            let len = buf.len();
            this.input.as_mut().consume(len);
        }
    }
}
