// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, bail, Error};
use async_trait::async_trait;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_fxfs::{CryptMarker, CryptProxy, KeyPurpose as FidlKeyPurpose};
use fxfs_crypto::{Crypt, KeyPurpose, UnwrappedKey, WrappedKey, WrappedKeyBytes, KEY_SIZE};

pub struct RemoteCrypt {
    client: CryptProxy,
}

impl RemoteCrypt {
    pub fn new(client: ClientEnd<CryptMarker>) -> Self {
        Self { client: client.into_proxy().unwrap() }
    }
}

trait IntoFidlKeyPurpose {
    fn into_fidl(self) -> FidlKeyPurpose;
}

impl IntoFidlKeyPurpose for KeyPurpose {
    fn into_fidl(self) -> FidlKeyPurpose {
        match self {
            KeyPurpose::Data => FidlKeyPurpose::Data,
            KeyPurpose::Metadata => FidlKeyPurpose::Metadata,
        }
    }
}

#[async_trait]
impl Crypt for RemoteCrypt {
    async fn create_key(
        &self,
        owner: u64,
        purpose: KeyPurpose,
    ) -> Result<(WrappedKey, UnwrappedKey), Error> {
        let (wrapping_key_id, key, unwrapped_key) =
            self.client.create_key(owner, purpose.into_fidl()).await?.map_err(|e| anyhow!(e))?;
        Ok((
            WrappedKey {
                wrapping_key_id: wrapping_key_id as u128,
                key: WrappedKeyBytes::try_from(key)?,
            },
            UnwrappedKey::new(
                unwrapped_key.try_into().map_err(|_| anyhow!("Unexpected unwrapped key length"))?,
            ),
        ))
    }

    async fn unwrap_key(
        &self,
        wrapped_key: &WrappedKey,
        owner: u64,
    ) -> Result<UnwrappedKey, Error> {
        let unwrapped = self
            .client
            // TODO(b/361105712): Remove try_into() when changing key to u128.
            .unwrap_key(wrapped_key.wrapping_key_id.try_into().map_err(
                |_| anyhow!("Wrapping key too large"))?, owner, &wrapped_key.key[..])
            .await?
            .map_err(|e| anyhow!(e))?;
        if unwrapped.len() != KEY_SIZE {
            bail!("Unexpected key length");
        }
        Ok(UnwrappedKey::new(unwrapped.try_into().unwrap()))
    }
}
