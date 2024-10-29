// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_examples_services as fexamples;
use fuchsia_component::server::ServiceFs;
use futures::lock::Mutex;
use futures::prelude::*;
use std::env;
use std::sync::Arc;
use tracing::*;

struct Account {
    /// Account owner's name.
    name: String,
    /// Account balance in cents.
    balance: i64,
}

#[fuchsia::main]
async fn main() {
    let mut args = env::args().skip(1);
    let name = args.next().expect("name arg");
    let balance = args.next().expect("balance arg").parse().expect("balance must be a number");
    info!(%name, %balance, "starting bank account provider");
    let account = Arc::new(Mutex::new(Account { name, balance }));

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service_instance("default", |req: fexamples::BankAccountRequest| req);
    fs.take_and_serve_directory_handle().expect("failed to serve outgoing namespace");
    fs.for_each_concurrent(None, move |request| {
        let account = account.clone();
        async move {
            match handle_request(account.clone(), request).await {
                Ok(()) => {}
                Err(err) => error!(%err, "failed to serve BankAccount request"),
            }
        }
    })
    .await;
}

async fn handle_request(
    account: Arc<Mutex<Account>>,
    request: fexamples::BankAccountRequest,
) -> Result<()> {
    match request {
        fexamples::BankAccountRequest::ReadOnly(mut stream) => {
            while let Some(request) =
                stream.try_next().await.context("failed to get next read-only request")?
            {
                let account = account.lock().await;
                match request {
                    fexamples::ReadOnlyAccountRequest::GetOwner { responder } => {
                        responder.send(&account.name).context("failed to send get_owner reply")?;
                    }
                    fexamples::ReadOnlyAccountRequest::GetBalance { responder } => {
                        responder
                            .send(account.balance)
                            .context("failed to send get_balance reply")?;
                    }
                }
            }
        }
        fexamples::BankAccountRequest::ReadWrite(mut stream) => {
            while let Some(request) =
                stream.try_next().await.context("failed to get next read-write request")?
            {
                let mut account = account.lock().await;
                match request {
                    fexamples::ReadWriteAccountRequest::GetOwner { responder } => {
                        responder.send(&account.name).context("failed to send get_owner reply")?;
                    }
                    fexamples::ReadWriteAccountRequest::GetBalance { responder } => {
                        responder
                            .send(account.balance)
                            .context("failed to send get_balance reply")?;
                    }
                    fexamples::ReadWriteAccountRequest::Debit { amount, responder } => {
                        let success = if account.balance >= amount {
                            account.balance -= amount;
                            true
                        } else {
                            false
                        };
                        info!(account = %account.name, balance = %account.balance, "balance updated");
                        responder.send(success).context("failed to send debit reply")?;
                    }
                    fexamples::ReadWriteAccountRequest::Credit { amount, responder } => {
                        account.balance += amount;
                        info!(account = %account.name, balance = %account.balance, "balance updated");
                        responder.send().context("failed to send credit reply")?;
                    }
                }
            }
        }
    }
    Ok(())
}
