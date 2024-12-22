// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::{Scope, Task};

use crate::protocol::{Client, ClientHandler, Responder, Server, ServerHandler, Transport};
use crate::{DecoderExt, WireString};

pub struct Ignore;

impl<T: Transport> ClientHandler<T> for Ignore {
    fn on_event(&mut self, _: u64, _: T::RecvBuffer) {}
}

impl<T: Transport> ServerHandler<T> for Ignore {
    fn on_event(&mut self, _: u64, _: T::RecvBuffer) {}
    fn on_transaction(&mut self, _: u64, _: T::RecvBuffer, _: Responder) {}
}

pub async fn test_close_on_drop<T: Transport>(client_end: T, server_end: T) {
    let (_, mut client_dispatcher) = Client::new(client_end);
    let client_task = Task::spawn(async move { client_dispatcher.run(Ignore).await });
    let (_, mut server_dispatcher) = Server::new(server_end);
    let server_task = Task::spawn(async move { server_dispatcher.run(Ignore).await });

    drop(client_task);
    server_task.await.expect("server encountered an error");
}

pub async fn test_send_receive<T: Transport>(client_end: T, server_end: T) {
    struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        fn on_event(&mut self, ordinal: u64, mut buffer: T::RecvBuffer) {
            assert_eq!(ordinal, 42);
            let message = T::decoder(&mut buffer)
                .decode_last::<WireString<'_>>()
                .expect("failed to decode request");
            assert_eq!(&**message, "Hello world");
        }

        fn on_transaction(&mut self, _: u64, _: T::RecvBuffer, _: Responder) {
            panic!("unexpected transaction");
        }
    }

    let (client, mut client_dispatcher) = Client::new(client_end);
    let client_task = Task::spawn(async move { client_dispatcher.run(Ignore).await });
    let (_, mut server_dispatcher) = Server::new(server_end);
    let server_task = Task::spawn(async move { server_dispatcher.run(TestServer).await });

    client
        .send_request(42, &mut "Hello world".to_string())
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request");
    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_transaction<T: Transport>(client_end: T, server_end: T) {
    struct TestServer<T: Transport> {
        server: Server<T>,
        scope: Scope,
    }

    impl<T: Transport> ServerHandler<T> for TestServer<T> {
        fn on_event(&mut self, _: u64, _: T::RecvBuffer) {
            panic!("unexpected event");
        }

        fn on_transaction(
            &mut self,
            ordinal: u64,
            mut buffer: T::RecvBuffer,
            responder: Responder,
        ) {
            let server = self.server.clone();
            self.scope.spawn(async move {
                assert_eq!(ordinal, 42);
                let message = T::decoder(&mut buffer)
                    .decode_last::<WireString<'_>>()
                    .expect("failed to decode request");
                assert_eq!(&**message, "Ping");

                server
                    .send_response(responder, 42, &mut "Pong".to_string())
                    .expect("failed to encode response")
                    .await
                    .expect("failed to send response");
            });
        }
    }

    let (client, mut client_dispatcher) = Client::new(client_end);
    let client_task = Task::spawn(async move { client_dispatcher.run(Ignore).await });
    let (server, mut server_dispatcher) = Server::new(server_end);
    let server_task = Task::spawn(async move {
        server_dispatcher.run(TestServer { server, scope: Scope::new() }).await
    });

    let mut buffer = client
        .send_transaction(42, &mut "Ping".to_string())
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request and receive response");
    let message =
        T::decoder(&mut buffer).decode_last::<WireString<'_>>().expect("failed to decode response");
    assert_eq!(&**message, "Pong");

    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_multiple_transactions<T: Transport>(client_end: T, server_end: T) {
    struct TestServer<T: Transport> {
        server: Server<T>,
        scope: Scope,
    }

    impl<T: Transport> ServerHandler<T> for TestServer<T> {
        fn on_event(&mut self, _: u64, _: T::RecvBuffer) {
            panic!("unexpected event");
        }

        fn on_transaction(
            &mut self,
            ordinal: u64,
            mut buffer: T::RecvBuffer,
            responder: Responder,
        ) {
            let server = self.server.clone();
            self.scope.spawn(async move {
                let message = T::decoder(&mut buffer)
                    .decode_last::<WireString<'_>>()
                    .expect("failed to decode request");

                let response = match ordinal {
                    1 => "One",
                    2 => "Two",
                    3 => "Three",
                    x => panic!("unexpected request ordinal {x} from client"),
                };

                assert_eq!(&**message, response);

                server
                    .send_response(responder, ordinal, &mut response.to_string())
                    .expect("server failed to encode response")
                    .await
                    .expect("server failed to send response");
            });
        }
    }

    let (client, mut client_dispatcher) = Client::new(client_end);
    let client_task = Task::spawn(async move { client_dispatcher.run(Ignore).await });
    let (server, mut server_dispatcher) = Server::new(server_end);
    let server_task = Task::spawn(async move {
        server_dispatcher.run(TestServer { server, scope: Scope::new() }).await
    });

    let send_one = client
        .send_transaction(1, &mut "One".to_string())
        .expect("client failed to encode request");
    let send_two = client
        .send_transaction(2, &mut "Two".to_string())
        .expect("client failed to encode request");
    let send_three = client
        .send_transaction(3, &mut "Three".to_string())
        .expect("client failed to encode request");
    let (response_one, response_two, response_three) =
        futures::join!(send_one, send_two, send_three);

    let mut buffer_one = response_one.expect("client failed to send request and receive response");
    let message_one = T::decoder(&mut buffer_one)
        .decode_last::<WireString<'_>>()
        .expect("failed to decode response");
    assert_eq!(&**message_one, "One");

    let mut buffer_two = response_two.expect("client failed to send request and receive response");
    let message_two = T::decoder(&mut buffer_two)
        .decode_last::<WireString<'_>>()
        .expect("failed to decode response");
    assert_eq!(&**message_two, "Two");

    let mut buffer_three =
        response_three.expect("client failed to send request and receive response");
    let message_three = T::decoder(&mut buffer_three)
        .decode_last::<WireString<'_>>()
        .expect("failed to decode response");
    assert_eq!(&**message_three, "Three");

    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_event<T: Transport>(client_end: T, server_end: T) {
    struct TestClient<T: Transport> {
        client: Client<T>,
    }

    impl<T: Transport> ClientHandler<T> for TestClient<T> {
        fn on_event(&mut self, ordinal: u64, mut buffer: T::RecvBuffer) {
            assert_eq!(ordinal, 10);
            let message = T::decoder(&mut buffer)
                .decode_last::<WireString<'_>>()
                .expect("failed to decode request");
            assert_eq!(&**message, "Surprise!");

            self.client.close();
        }
    }

    let (client, mut client_dispatcher) = Client::new(client_end);
    let client_task =
        Task::spawn(async move { client_dispatcher.run(TestClient { client }).await });
    let (server, mut server_dispatcher) = Server::new(server_end);
    let server_task = Task::spawn(async move { server_dispatcher.run(Ignore).await });

    server
        .send_event(10, &mut "Surprise!".to_string())
        .expect("server failed to encode response")
        .await
        .expect("server failed to send response");

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}
