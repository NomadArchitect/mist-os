// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{AnyHandle, Client, FDomainTransport};
use fdomain_container::wire::FDomainCodec;
use fdomain_container::FDomain;
use fidl_fuchsia_fdomain_ext::AsFDomainRights;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

struct LocalFDomain(FDomainCodec);

impl LocalFDomain {
    fn new_client() -> Arc<Client> {
        let (client, fut) = Client::new(LocalFDomain(FDomainCodec::new(FDomain::new_empty())));
        fuchsia_async::Task::spawn(fut).detach();
        client
    }
}

impl FDomainTransport for LocalFDomain {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        _ctx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(self.0.message(msg).map_err(std::io::Error::other))
    }
}

impl Stream for LocalFDomain {
    type Item = Result<Box<[u8]>, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx).map_err(std::io::Error::other)
    }
}

#[fuchsia::test]
async fn socket() {
    let client = LocalFDomain::new_client();

    let (a, b) = client.create_stream_socket().await.unwrap();
    let test_str = b"Feral Cats Move In Mysterious Ways";

    a.write_all(test_str).await.unwrap();

    let mut got = Vec::with_capacity(test_str.len());

    while got.len() < test_str.len() {
        got.append(&mut b.read(test_str.len() - got.len()).await.unwrap());
    }

    assert_eq!(test_str, got.as_slice());
}

#[fuchsia::test]
async fn channel() {
    let client = LocalFDomain::new_client();

    let (a, b) = client.create_channel().await.unwrap();
    let (c, d) = client.create_stream_socket().await.unwrap();
    let test_str_1 = b"Feral Cats Move In Mysterious Ways";
    let test_str_2 = b"Joyous Throbbing! Jubilant Pulsing!";

    a.write(test_str_1, vec![c.into()]).await.unwrap();
    d.write_all(test_str_2).await.unwrap();

    let mut msg = b.recv_msg().await.unwrap();

    assert_eq!(test_str_1, msg.bytes.as_slice());

    let handle = msg.handles.pop().unwrap();
    assert!(msg.handles.is_empty());

    let expect_rights = fidl::Rights::DUPLICATE
        | fidl::Rights::TRANSFER
        | fidl::Rights::READ
        | fidl::Rights::WRITE
        | fidl::Rights::GET_PROPERTY
        | fidl::Rights::SET_PROPERTY
        | fidl::Rights::SIGNAL
        | fidl::Rights::SIGNAL_PEER
        | fidl::Rights::WAIT
        | fidl::Rights::INSPECT
        | fidl::Rights::MANAGE_SOCKET;
    assert_eq!(expect_rights.as_fdomain_rights().unwrap(), handle.rights);

    let AnyHandle::Socket(e) = handle.handle else { panic!() };

    let mut got = Vec::with_capacity(test_str_2.len());

    while got.len() < test_str_2.len() {
        got.append(&mut e.read(test_str_2.len() - got.len()).await.unwrap());
    }

    assert_eq!(test_str_2, got.as_slice());
}

#[fuchsia::test]
async fn socket_async() {
    let client = LocalFDomain::new_client();

    let (a, b) = client.create_stream_socket().await.unwrap();
    let test_str_a = b"Feral Cats Move In Mysterious Ways";
    let test_str_b = b"Almost all of our feelings were programmed in to us.";

    let (mut b_reader, b_writer) = b.stream().unwrap();
    b_writer.write_all(test_str_a).await.unwrap();

    let write_side = async move {
        let mut got = Vec::with_capacity(test_str_a.len());

        while got.len() < test_str_a.len() {
            got.append(&mut a.read(test_str_a.len() - got.len()).await.unwrap());
        }

        assert_eq!(test_str_a, got.as_slice());

        for _ in 0..5 {
            a.write_all(test_str_a).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
            a.write_all(test_str_b).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
        }
    };

    let read_side = async move {
        let mut buf = Vec::new();
        buf.resize((test_str_a.len() + test_str_b.len()) * 5, 0);

        for mut buf in buf.chunks_mut(20) {
            while !buf.is_empty() {
                let len = b_reader.read(buf).await.unwrap();
                buf = &mut buf[len..];
            }
        }

        let mut buf = buf.as_mut_slice();

        for _ in 0..5 {
            assert!(buf.starts_with(test_str_a));
            buf = &mut buf[test_str_a.len()..];
            assert!(buf.starts_with(test_str_b));
            buf = &mut buf[test_str_b.len()..];
        }

        assert!(buf.is_empty());
    };

    futures::future::join(read_side, write_side).await;
}
