// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{bail, Error};
use fidl::HandleRef;
use fidl_fuchsia_overnet_protocol::{ChannelRights, EventPairRights, SocketRights, SocketType};

#[cfg(target_os = "fuchsia")]
use fidl::AsHandleRef;

#[cfg(not(target_os = "fuchsia"))]
use fidl::EmulatedHandleRef;

#[cfg(target_os = "fuchsia")]
pub(crate) type HandleKey = zx::Koid;

#[cfg(not(target_os = "fuchsia"))]
pub(crate) type HandleKey = u64;

/// When sending a datagram on a channel, contains information needed to establish streams
/// for any handles being sent.
#[derive(Copy, Clone, Debug)]
pub(crate) enum HandleType {
    /// A handle of type channel is being sent.
    Channel(ChannelRights),
    Socket(SocketType, SocketRights),
    EventPair,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct HandleInfo {
    pub(crate) handle_type: HandleType,
    pub(crate) this_handle_key: HandleKey,
    pub(crate) pair_handle_key: HandleKey,
}

#[cfg(not(target_os = "fuchsia"))]
pub(crate) fn handle_info(hdl: HandleRef<'_>) -> Result<HandleInfo, Error> {
    let handle_type = match hdl.object_type() {
        fidl::ObjectType::CHANNEL => {
            HandleType::Channel(ChannelRights::READ | ChannelRights::WRITE)
        }
        fidl::ObjectType::SOCKET => {
            HandleType::Socket(SocketType::Stream, SocketRights::READ | SocketRights::WRITE)
        }
        fidl::ObjectType::EVENTPAIR => HandleType::EventPair,
        _ => bail!("Unsupported handle type"),
    };
    let (this_handle_key, pair_handle_key) = hdl.koid_pair();
    Ok(HandleInfo { handle_type, this_handle_key, pair_handle_key })
}

#[cfg(target_os = "fuchsia")]
pub(crate) fn handle_info(handle: HandleRef<'_>) -> Result<HandleInfo, Error> {
    let basic_info = handle.basic_info()?;

    let handle_type = match basic_info.object_type {
        zx::ObjectType::CHANNEL => {
            let mut rights = ChannelRights::empty();
            rights.set(ChannelRights::READ, basic_info.rights.contains(zx::Rights::READ));
            rights.set(ChannelRights::WRITE, basic_info.rights.contains(zx::Rights::WRITE));
            HandleType::Channel(rights)
        }
        zx::ObjectType::SOCKET => {
            let socket = handle.cast::<zx::Socket>();
            let info = socket.info()?;
            let socket_type = match info.options {
                zx::SocketOpts::STREAM => SocketType::Stream,
                zx::SocketOpts::DATAGRAM => SocketType::Datagram,
                _ => bail!("Unhandled socket options"),
            };
            let mut rights = SocketRights::empty();
            rights.set(SocketRights::READ, basic_info.rights.contains(zx::Rights::READ));
            rights.set(SocketRights::WRITE, basic_info.rights.contains(zx::Rights::WRITE));
            HandleType::Socket(socket_type, rights)
        }
        zx::ObjectType::EVENTPAIR => HandleType::EventPair,
        _ => bail!("Handle type not proxyable {:?}", handle.basic_info()?.object_type),
    };

    Ok(HandleInfo {
        handle_type,
        this_handle_key: basic_info.koid,
        pair_handle_key: basic_info.related_koid,
    })
}

pub(crate) trait WithRights {
    type Rights;
    fn with_rights(self, rights: Self::Rights) -> Result<Self, Error>
    where
        Self: Sized;
}

#[cfg(target_os = "fuchsia")]
impl WithRights for fidl::Channel {
    type Rights = ChannelRights;
    fn with_rights(self, rights: ChannelRights) -> Result<Self, Error> {
        use zx::HandleBased;
        let mut zx_rights = self.basic_info()?.rights;
        zx_rights.set(zx::Rights::READ, rights.contains(ChannelRights::READ));
        zx_rights.set(zx::Rights::WRITE, rights.contains(ChannelRights::WRITE));
        zx_rights.insert(zx::Rights::TRANSFER);
        Ok(self.replace_handle(zx_rights)?)
    }
}

#[cfg(target_os = "fuchsia")]
impl WithRights for fidl::Socket {
    type Rights = SocketRights;
    fn with_rights(self, rights: SocketRights) -> Result<Self, Error> {
        use zx::HandleBased;
        let mut zx_rights = self.basic_info()?.rights;
        zx_rights.set(zx::Rights::READ, rights.contains(SocketRights::READ));
        zx_rights.set(zx::Rights::WRITE, rights.contains(SocketRights::WRITE));
        zx_rights.insert(zx::Rights::TRANSFER);
        Ok(self.replace_handle(zx_rights)?)
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl WithRights for fidl::Channel {
    type Rights = ChannelRights;
    fn with_rights(self, rights: ChannelRights) -> Result<Self, Error> {
        if rights != ChannelRights::READ | ChannelRights::WRITE {
            bail!("Restricted rights not supported on non-Fuchsia platforms");
        }
        Ok(self)
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl WithRights for fidl::Socket {
    type Rights = SocketRights;
    fn with_rights(self, rights: SocketRights) -> Result<Self, Error> {
        if rights != SocketRights::READ | SocketRights::WRITE {
            bail!("Restricted rights not supported on non-Fuchsia platforms");
        }
        Ok(self)
    }
}

impl WithRights for fidl::EventPair {
    type Rights = EventPairRights;
    fn with_rights(self, rights: EventPairRights) -> Result<Self, Error> {
        if !rights.is_empty() {
            bail!("Non-empty rights ({:?}) not supported for event pair", rights);
        }
        Ok(self)
    }
}
