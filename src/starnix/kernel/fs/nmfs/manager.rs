// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::nmfs::NetworkMessage;
use bstr::BString;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_inspect_derive::{IValue, Inspect, Unit, WithInspect};
use once_cell::sync::OnceCell;
use starnix_logging::{log_error, log_info};
use starnix_sync::{Mutex, MutexGuard};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use {fidl_fuchsia_netpol_socketproxy as fnp_socketproxy, fuchsia_zircon as zx};

/// Manager for communicating network properties.
#[derive(Inspect, Default)]
pub(crate) struct NetworkManager {
    // TODO(https://fxbug.dev/348639637): Change this to a Mutex-wrapped
    // Option so that the proxy can be re-initialized.
    starnix_networks: OnceCell<fnp_socketproxy::StarnixNetworksSynchronousProxy>,
    // Timeout for thread-blocking calls to the socketproxy.
    proxy_timeout: zx::Duration,
    #[inspect(forward)]
    inner: Mutex<IValue<NetworkManagerInner>>,
}

#[derive(Unit, Default)]
struct NetworkManagerInner {
    // Keeps track of networks and their [`NetworkMessage`].
    #[inspect(skip)]
    default_id: Option<u32>,
    #[inspect(skip)]
    networks: HashMap<u32, Option<NetworkMessage>>,

    default_ids_set: SeenSentData,
    added_networks: SeenSentData,
    updated_networks: SeenSentData,
    removed_networks: SeenSentData,
}

#[derive(Unit, Default)]
struct SeenSentData {
    // The number of event occurrences witnessed
    // by the NetworkManager.
    seen: u64,
    // The number of event occurrences that have been
    // sent successfully to the socketproxy.
    sent: u64,
}

// This is an Arc so that the Kernel can hold a reference to the
// NetworkManager across all of its threads.
#[derive(Clone)]
pub struct NetworkManagerHandle(pub(crate) Arc<NetworkManager>);

impl NetworkManagerHandle {
    /// Create a NetworkManagerHandle with a `nmfs` inspect node.
    pub fn new_with_inspect(node: &fuchsia_inspect::Node) -> Self {
        Self(Arc::new(
            NetworkManager { proxy_timeout: zx::Duration::from_seconds(1), ..Default::default() }
                .with_inspect(node, "nmfs")
                .expect("Failed to attach 'nmfs' node"),
        ))
    }

    /// Initialize the connection to the socketproxy.
    pub fn init(&self) -> Result<(), anyhow::Error> {
        self.0.init()
    }
}

impl NetworkManager {
    fn init(&self) -> Result<(), anyhow::Error> {
        let starnix_networks =
            connect_to_protocol_sync::<fnp_socketproxy::StarnixNetworksMarker>()?;

        self.starnix_networks
            .set(starnix_networks)
            .expect("Starnix Networks should be uninitialized");

        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, IValue<NetworkManagerInner>> {
        self.inner.lock()
    }

    pub(crate) fn get_default_network_id(&self) -> Option<u32> {
        self.lock().default_id
    }

    pub(crate) fn get_network(&self, network_id: &u32) -> Option<Option<NetworkMessage>> {
        self.lock().networks.get(network_id).cloned()
    }

    pub(crate) fn get_default_id_as_bytes(&self) -> BString {
        let default_id = match self.get_default_network_id() {
            Some(id) => id.to_string(),
            None => "".to_string(),
        };
        default_id.into_bytes().into()
    }

    pub(crate) fn get_network_by_id_as_bytes(&self, network_id: u32) -> BString {
        let network_info = match self.get_network(&network_id) {
            Some(Some(network)) => serde_json::to_string(&network).unwrap_or("{}".to_string()),
            // A network with that was created but hasn't yet
            // been populated with network properties.
            Some(None) | None => "{}".to_string(),
        };
        network_info.into_bytes().into()
    }

    // Set the default network identifier. Propagate the change to the
    // socketproxy when present.
    pub(crate) fn set_default_network_id(&self, network_id: Option<u32>) {
        {
            let mut inner_guard = self.lock();
            let mut inner = inner_guard.as_mut();
            inner.default_id = network_id;
            inner.default_ids_set.seen += 1;
        }

        // Only log when there is an internal proxy error. The `ProxyNotInitialized`
        // variant likely means that the `network_manager` feature is disabled.
        match self.fidl_set_default_network_id(network_id) {
            Ok(()) => {
                log_info!(
                    "Successfully set network with id {network_id:?} as default in socketproxy",
                );
                let mut inner_guard = self.lock();
                inner_guard.as_mut().default_ids_set.sent += 1;
            }
            Err(NetworkManagerError::ProxyNotInitialized) => {}
            Err(e) => {
                log_error!(
                    "Failed to set network with id {network_id:?} as default in socketproxy; {e:?}"
                );
            }
        }
    }

    // Populate a new element in the Map. This does not
    // propagate to the socketproxy.
    //
    // An error will be returned if a network with the id
    // exists in the local state.
    pub(crate) fn add_empty_network(&self, network_id: u32) -> Result<(), Errno> {
        let mut inner_guard = self.lock();
        match inner_guard.as_mut().networks.entry(network_id) {
            Entry::Occupied(_) => {
                log_error!(
                    "Failed to add empty network to HashMap, was present for id: {}",
                    network_id
                );
                return Err(errno!(EEXIST));
            }
            Entry::Vacant(entry) => entry.insert(None),
        };
        Ok(())
    }

    // Add a new network. Propagate the change to the
    // socketproxy when present.
    //
    // An error will be returned if a network with the id
    // exists in the local state.
    pub(crate) fn add_network(&self, network: NetworkMessage) -> Result<(), Errno> {
        {
            let mut inner_guard = self.lock();
            let mut inner = inner_guard.as_mut();
            match inner.networks.entry(network.netid) {
                Entry::Occupied(mut entry) => {
                    // This is deliberately before any Map manipulation because we
                    // should not modify the Map state if we return an error.
                    if let Some(network) = entry.get() {
                        log_error!(
                            "Failed to add network with id {} to HashMap, already existed",
                            network.netid
                        );
                        return Err(errno!(EEXIST));
                    }
                    let _ = entry.insert(Some(network.clone()));
                }
                Entry::Vacant(entry) => {
                    let _ = entry.insert(Some(network.clone()));
                }
            }
            inner.added_networks.seen += 1;
        }

        // Only log when there is an internal proxy error. The `ProxyNotInitialized`
        // variant likely means that the `network_manager` feature is disabled.
        match self.fidl_add_network((&network).into()) {
            Ok(()) => {
                log_info!("Successfully added network with id {} to socketproxy", network.netid);
                let mut inner_guard = self.lock();
                inner_guard.as_mut().added_networks.sent += 1;
            }
            Err(NetworkManagerError::ProxyNotInitialized) => {}
            Err(e) => {
                log_error!("Failed to add network with id {} to socketproxy; {e:?}", network.netid);
            }
        }

        Ok(())
    }

    // Update an existing network. Propagate the change to the
    // socketproxy when present.
    //
    // An error will be returned if a network with the id does not
    // exist in the local state.
    pub(crate) fn update_network(&self, network: NetworkMessage) -> Result<(), Errno> {
        {
            let mut inner_guard = self.lock();
            let mut inner = inner_guard.as_mut();
            // Ensure that there is a network already present at that netid
            // prior to modifying the map.
            let _old_network = match inner.networks.entry(network.netid) {
                Entry::Occupied(mut entry) => {
                    if let None = entry.get() {
                        return Err(errno!(ENOENT));
                    }
                    entry.insert(Some(network.clone()))
                }
                Entry::Vacant(_) => {
                    return Err(errno!(ENOENT));
                }
            };
            inner.updated_networks.seen += 1;
        };

        // Only log when there is an internal proxy error. The `ProxyNotInitialized`
        // variant likely means that the `network_manager` feature is disabled.
        match self.fidl_update_network((&network).into()) {
            Ok(()) => {
                log_info!("Successfully updated network with id {} in socketproxy", network.netid);
                let mut inner_guard = self.lock();
                inner_guard.as_mut().updated_networks.sent += 1;
            }
            Err(NetworkManagerError::ProxyNotInitialized) => {}
            Err(e) => {
                log_error!(
                    "Failed to update network with id {} in socketproxy; {e:?}",
                    network.netid
                );
            }
        }

        Ok(())
    }

    // Remove an existing network. Propagate the change to the
    // socketproxy when present.
    //
    // An error will be returned if a network with the id does not
    // exist in the local state.
    pub(crate) fn remove_network(&self, network_id: u32) -> Result<(), Errno> {
        // Surface an error if the network is the current default
        // network or if the network is not found.
        let default_network_id = self.get_default_network_id();
        if let Some(id) = default_network_id {
            if id == network_id {
                return Err(errno!(EPERM));
            }
        }
        {
            let mut inner_guard = self.lock();
            let mut inner = inner_guard.as_mut();
            if let None = inner.networks.remove(&network_id) {
                return Err(errno!(ENOENT));
            }
            inner.removed_networks.seen += 1;
        }

        // Only log when there is an internal proxy error. The `ProxyNotInitialized`
        // variant likely means that the `network_manager` feature is disabled.
        match self.fidl_remove_network(&network_id) {
            Ok(()) => {
                log_info!("Successfully removed network with id {network_id} from socketproxy",);
                let mut inner_guard = self.lock();
                inner_guard.as_mut().removed_networks.sent += 1;
            }
            Err(NetworkManagerError::ProxyNotInitialized) => {}
            Err(e) => {
                log_error!("Failed to remove network with id {network_id} in socketproxy; {e:?}");
            }
        }
        Ok(())
    }
}

// The functions that propagate calls to the socketproxy prioritize maintaining
// a correct version of local state and logging an error if the socketproxy
// state is not aligned. By maintaining a Map of network properties
// `NetworkManager` can replay the networks to the socketproxy in the case of
// the component restarting.
impl NetworkManager {
    fn starnix_networks(
        &self,
    ) -> Result<&fnp_socketproxy::StarnixNetworksSynchronousProxy, NetworkManagerError> {
        match self.starnix_networks.get() {
            Some(p) => Ok(p),
            None => Err(NetworkManagerError::ProxyNotInitialized),
        }
    }

    // Call `set_default` on `StarnixNetworks`.
    fn fidl_set_default_network_id(
        &self,
        network_id: Option<u32>,
    ) -> Result<(), NetworkManagerError> {
        let starnix_networks = self.starnix_networks()?;
        let network_id = match network_id {
            Some(id) => fidl_fuchsia_posix_socket::OptionalUint32::Value(id),
            None => {
                fidl_fuchsia_posix_socket::OptionalUint32::Unset(fidl_fuchsia_posix_socket::Empty)
            }
        };
        Ok(starnix_networks
            .set_default(&network_id, zx::MonotonicTime::after(self.proxy_timeout))??)
    }

    // Call `add` on `StarnixNetworks`.
    fn fidl_add_network(
        &self,
        network: fnp_socketproxy::Network,
    ) -> Result<(), NetworkManagerError> {
        let starnix_networks = self.starnix_networks()?;
        Ok(starnix_networks.add(&network, zx::MonotonicTime::after(self.proxy_timeout))??)
    }

    // Call `update` on `StarnixNetworks`.
    fn fidl_update_network(
        &self,
        network: fnp_socketproxy::Network,
    ) -> Result<(), NetworkManagerError> {
        let starnix_networks = self.starnix_networks()?;
        Ok(starnix_networks.update(&network, zx::MonotonicTime::after(self.proxy_timeout))??)
    }

    // Call `remove` on `StarnixNetworks`.
    fn fidl_remove_network(&self, network_id: &u32) -> Result<(), NetworkManagerError> {
        let starnix_networks = self.starnix_networks()?;
        Ok(starnix_networks.remove(*network_id, zx::MonotonicTime::after(self.proxy_timeout))??)
    }
}

// Errors produced when communicating updates to
// the socket proxy.
#[derive(Clone, Debug, Error)]
pub(crate) enum NetworkManagerError {
    #[error("Error during socketproxy Add: {0:?}")]
    Add(fnp_socketproxy::NetworkRegistryAddError),
    #[error("Error calling FIDL on socketproxy: {0:?}")]
    Fidl(#[from] fidl::Error),
    #[error("Proxy was not initialized prior to use")]
    ProxyNotInitialized,
    #[error("Error during socketproxy Remove: {0:?}")]
    Remove(fnp_socketproxy::NetworkRegistryRemoveError),
    #[error("Error during socketproxy SetDefault: {0:?}")]
    SetDefault(fnp_socketproxy::NetworkRegistrySetDefaultError),
    #[error("Error during socketproxy Update: {0:?}")]
    Update(fnp_socketproxy::NetworkRegistryUpdateError),
}

impl From<fnp_socketproxy::NetworkRegistryAddError> for NetworkManagerError {
    fn from(error: fnp_socketproxy::NetworkRegistryAddError) -> Self {
        NetworkManagerError::Add(error)
    }
}

impl From<fnp_socketproxy::NetworkRegistryRemoveError> for NetworkManagerError {
    fn from(error: fnp_socketproxy::NetworkRegistryRemoveError) -> Self {
        NetworkManagerError::Remove(error)
    }
}

impl From<fnp_socketproxy::NetworkRegistrySetDefaultError> for NetworkManagerError {
    fn from(error: fnp_socketproxy::NetworkRegistrySetDefaultError) -> Self {
        NetworkManagerError::SetDefault(error)
    }
}

impl From<fnp_socketproxy::NetworkRegistryUpdateError> for NetworkManagerError {
    fn from(error: fnp_socketproxy::NetworkRegistryUpdateError) -> Self {
        NetworkManagerError::Update(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use futures::StreamExt as _;
    use starnix_uapi::{EEXIST, ENOENT, EPERM};
    use test_case::test_case;

    fn test_network_message_from_id(netid: u32) -> NetworkMessage {
        NetworkMessage { netid, ..Default::default() }
    }

    #[::fuchsia::test]
    fn test_set_default_network_id() {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &NetworkManagerHandle::new_with_inspect(inspector.root()).0;

        let default: Option<u32> = Some(1);
        manager.set_default_network_id(default);
        assert_eq!(manager.get_default_network_id(), default);
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        let default: Option<u32> = None;
        manager.set_default_network_id(default);
        assert_eq!(manager.get_default_network_id(), default);
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 2u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[::fuchsia::test]
    fn test_add_network() {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &NetworkManagerHandle::new_with_inspect(inspector.root()).0;

        let network1 = test_network_message_from_id(1);
        assert_matches!(manager.add_network(network1.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Ensure we cannot add a network with the same id twice.
        assert_matches!(
            manager.add_network(network1),
            Err(errno) => errno.code.error_code() == EEXIST
        );

        // Ensure we can add a network with a different id.
        let network2 = test_network_message_from_id(2);
        assert_matches!(manager.add_network(network2.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 2u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[::fuchsia::test]
    fn test_update_network() {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &NetworkManagerHandle::new_with_inspect(inspector.root()).0;

        let network_id = 1;
        let network_initial = test_network_message_from_id(network_id);
        // Ensure we cannot update a network that doesn't exist.
        assert_matches!(
            manager.update_network(network_initial.clone()),
            Err(errno) => errno.code.error_code() == ENOENT
        );

        // Insert the network manually and then update the network. The
        // retrieved network should have the same properties as the
        // network used to update the entry.
        {
            let mut inner_guard = manager.lock();
            let _ = inner_guard.as_mut().networks.insert(network_id, Some(network_initial.clone()));
        }
        let new_mark = 1;
        let mut network_new = network_initial.clone();
        network_new.mark = new_mark;
        assert_matches!(manager.update_network(network_new.clone()), Ok(()));
        assert_matches!(
            manager.get_network(&network_id),
            Some(Some(network_new)) => network_new.mark == new_mark
        );
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                updated_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[::fuchsia::test]
    fn test_add_empty_network() {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &NetworkManagerHandle::new_with_inspect(inspector.root()).0;

        let network_id = 1;
        assert_matches!(manager.add_empty_network(network_id), Ok(()));

        // Ensure we cannot add an empty network with the same id twice.
        assert_matches!(
            manager.add_empty_network(network_id),
            Err(errno) => errno.code.error_code() == EEXIST
        );
        assert_matches!(manager.get_network(&network_id), Some(None));
        // Empty networks don't get sent to the socketproxy, so they are
        // ignored in SeenSentData.
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[::fuchsia::test]
    fn test_remove_network() {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &NetworkManagerHandle::new_with_inspect(inspector.root()).0;

        let network_id = 1;
        let network = test_network_message_from_id(network_id);
        // Ensure we cannot remove a network that doesn't exist.
        assert_matches!(
            manager.remove_network(network_id),
            Err(errno) => errno.code.error_code() == ENOENT
        );

        // Insert the network manually and then remove it.
        {
            let mut inner_guard = manager.lock();
            let _ = inner_guard.as_mut().networks.insert(network_id, Some(network.clone()));
        }
        assert_matches!(manager.remove_network(network_id), Ok(()));

        // Ensure we cannot remove the network again.
        assert_matches!(
            manager.remove_network(network_id),
            Err(errno) => errno.code.error_code() == ENOENT
        );
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                removed_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[::fuchsia::test]
    fn test_multiple_operations() {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &NetworkManagerHandle::new_with_inspect(inspector.root()).0;

        // Add a network.
        let network1 = test_network_message_from_id(1);
        assert_matches!(manager.add_network(network1.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Set the default network.
        manager.set_default_network_id(Some(1));
        assert_eq!(manager.get_default_network_id(), Some(1));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Update the network.
        let mut network1_updated = network1.clone();
        network1_updated.mark = 1;
        assert_matches!(manager.update_network(network1_updated.clone()), Ok(()));
        assert_matches!(
            manager.get_network(&network1.netid),
            Some(Some(network)) => network.mark == 1
        );
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                updated_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Add another network.
        let network2 = test_network_message_from_id(2);
        assert_matches!(manager.add_network(network2.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 2u64,
                    sent: 0u64,
                },
            },
        });

        // Attempt to remove the first network. We should get an error
        // since network1 is the current default network.
        assert_matches!(
            manager.remove_network(network1.netid),
            Err(errno) => errno.code.error_code() == EPERM
        );

        // Ensure the first network still exists.
        assert_eq!(manager.get_network(&network1.netid), Some(Some(network1_updated.clone())));

        // Set the default network to the second network.
        manager.set_default_network_id(Some(2));
        assert_eq!(manager.get_default_network_id(), Some(2));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 2u64,
                    sent: 0u64,
                },
            },
        });

        // Remove the first network as it is no longer the default network.
        assert_matches!(manager.remove_network(network1.netid), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: {
                default_ids_set: {
                    seen: 2u64,
                    sent: 0u64,
                },
                added_networks: {
                    seen: 2u64,
                    sent: 0u64,
                },
                updated_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
                removed_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });
    }

    // Create a NetworkManagerHandle for use with testing. Use an infinite
    // timeout because we know that the mock socketproxy is guaranteed to send
    // a response for every request.
    fn create_test_network_manager_handle(node: &fuchsia_inspect::Node) -> NetworkManagerHandle {
        NetworkManagerHandle(Arc::new(
            NetworkManager { proxy_timeout: zx::Duration::INFINITE, ..Default::default() }
                .with_inspect(node, "nmfs")
                .expect("Failed to attach 'nmfs' node"),
        ))
    }

    // Set the `StarnixNetworksMarker` in the NetworkManager and mock out
    // the responses to `StarnixNetworksRequest`s with provided results.
    fn setup_proxy(
        inspect_node: &fuchsia_inspect::Node,
        results: Vec<Result<(), NetworkManagerError>>,
    ) -> NetworkManagerHandle {
        let manager = create_test_network_manager_handle(inspect_node);
        let (proxy, mut stream) = fidl::endpoints::create_sync_proxy_and_stream::<
            fnp_socketproxy::StarnixNetworksMarker,
        >()
        .unwrap();
        manager.0.starnix_networks.set(proxy).expect("Starnix Networks should be uninitialized");

        let mut results = results.into_iter();
        fuchsia_async::Task::spawn(async move {
            while let Some(item) = stream.next().await {
                let result = results
                    .next()
                    .expect("there should be an equivalent # of results and requests");
                match item.expect("receive request") {
                    fnp_socketproxy::StarnixNetworksRequest::SetDefault {
                        network_id: _,
                        responder,
                    } => {
                        let res = result.map_err(|e| match e {
                            NetworkManagerError::SetDefault(err) => err,
                            _ => unreachable!("should have been SetDefault error variant"),
                        });
                        responder.send(res).expect("respond to SetDefault");
                    }
                    fnp_socketproxy::StarnixNetworksRequest::Add { network: _, responder } => {
                        let res = result.map_err(|e| match e {
                            NetworkManagerError::Add(err) => err,
                            _ => unreachable!("should have been Add error variant"),
                        });
                        responder.send(res).expect("respond to Add");
                    }
                    fnp_socketproxy::StarnixNetworksRequest::Update { network: _, responder } => {
                        let res = result.map_err(|e| match e {
                            NetworkManagerError::Update(err) => err,
                            _ => unreachable!("should have been Update error variant"),
                        });
                        responder.send(res).expect("respond to Update");
                    }
                    fnp_socketproxy::StarnixNetworksRequest::Remove {
                        network_id: _,
                        responder,
                    } => {
                        let res = result.map_err(|e| match e {
                            NetworkManagerError::Remove(err) => err,
                            _ => unreachable!("should have been Remove error variant"),
                        });
                        responder.send(res).expect("respond to Remove");
                    }
                }
            }
        })
        .detach();
        manager
    }

    // Mock of private `manager::SeenSentData` to use for
    // improved test case readability.
    struct SeenSentData {
        seen: u64,
        sent: u64,
    }

    #[test_case(vec![Ok(()), Ok(())], SeenSentData { seen: 2, sent: 2 }; "all_success")]
    #[test_case(
        vec![
            Ok(()),
            Err(NetworkManagerError::SetDefault(fnp_socketproxy::NetworkRegistrySetDefaultError::NotFound)),
        ],
        SeenSentData { seen: 2, sent: 1 };
        "one_error"
    )]
    #[test_case(
        vec![
            Err(NetworkManagerError::SetDefault(fnp_socketproxy::NetworkRegistrySetDefaultError::NotFound)),
            Err(NetworkManagerError::SetDefault(fnp_socketproxy::NetworkRegistrySetDefaultError::NotFound)),
        ],
        SeenSentData { seen: 2, sent: 0 };
        "all_errors"
    )]
    #[::fuchsia::test(threads = 2)]
    async fn test_set_default_network_id_with_proxy(
        results: Vec<Result<(), NetworkManagerError>>,
        expected_data: SeenSentData,
    ) {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &setup_proxy(inspector.root(), results).0;

        manager.set_default_network_id(Some(1));
        assert_eq!(manager.get_default_network_id(), Some(1));

        manager.set_default_network_id(Some(2));
        assert_eq!(manager.get_default_network_id(), Some(2));

        assert_data_tree!(inspector, root: {
            nmfs: {
                default_ids_set: {
                    seen: expected_data.seen,
                    sent: expected_data.sent,
                },
                added_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
                updated_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
                removed_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[test_case(vec![Ok(()), Ok(())], SeenSentData { seen: 2, sent: 2 }; "all_success")]
    #[test_case(
        vec![
            Ok(()),
            Err(NetworkManagerError::Add(fnp_socketproxy::NetworkRegistryAddError::DuplicateNetworkId)),
        ],
        SeenSentData { seen: 2, sent: 1 };
        "one_error"
    )]
    #[test_case(
        vec![
            Err(NetworkManagerError::Add(fnp_socketproxy::NetworkRegistryAddError::MissingNetworkId)),
            Err(NetworkManagerError::Add(fnp_socketproxy::NetworkRegistryAddError::MissingNetworkInfo)),
        ],
        SeenSentData { seen: 2, sent: 0 };
        "all_errors"
    )]
    #[::fuchsia::test(threads = 2)]
    async fn test_add_network_with_proxy(
        results: Vec<Result<(), NetworkManagerError>>,
        expected_data: SeenSentData,
    ) {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &setup_proxy(inspector.root(), results).0;

        // `add_network` returns Ok(()) as long as the network
        // addition is applied locally, regardless of if the call
        // is sent to the socketproxy successfully.
        let network1 = test_network_message_from_id(1);
        assert_matches!(manager.add_network(network1.clone()), Ok(()));

        let network2 = test_network_message_from_id(2);
        assert_matches!(manager.add_network(network2.clone()), Ok(()));

        assert_data_tree!(inspector, root: {
            nmfs: {
                default_ids_set: {
                    seen: 0u64,
                    sent: 0u64,
                },
                added_networks: {
                    seen: expected_data.seen,
                    sent: expected_data.sent,
                },
                updated_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
                removed_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[test_case(vec![Ok(()), Ok(())], SeenSentData { seen: 2, sent: 2 }; "all_success")]
    #[test_case(
        vec![
            Ok(()),
            Err(NetworkManagerError::Update(fnp_socketproxy::NetworkRegistryUpdateError::MissingNetworkId)),
        ],
        SeenSentData { seen: 2, sent: 1 };
        "one_error"
    )]
    #[test_case(
        vec![
            Err(NetworkManagerError::Update(fnp_socketproxy::NetworkRegistryUpdateError::NotFound)),
            Err(NetworkManagerError::Update(fnp_socketproxy::NetworkRegistryUpdateError::MissingNetworkInfo)),
        ],
        SeenSentData { seen: 2, sent: 0 };
        "all_errors"
    )]
    #[::fuchsia::test(threads = 2)]
    async fn test_update_network_with_proxy(
        results: Vec<Result<(), NetworkManagerError>>,
        expected_data: SeenSentData,
    ) {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &setup_proxy(inspector.root(), results).0;

        let network_id = 1;
        let network = test_network_message_from_id(network_id);

        // Insert the network manually and then update the network.
        {
            let mut inner_guard = manager.lock();
            let _ = inner_guard.as_mut().networks.insert(network_id, Some(network.clone()));
        }

        // `update_network` returns Ok(()) as long as the network
        // update is applied locally, regardless of if the call
        // is sent to the socketproxy successfully. Use the same
        // network information -- another test verifies that the
        // change is applied successfully.
        assert_matches!(manager.update_network(network.clone()), Ok(()));
        assert_matches!(manager.update_network(network), Ok(()));

        assert_data_tree!(inspector, root: {
            nmfs: {
                default_ids_set: {
                    seen: 0u64,
                    sent: 0u64,
                },
                added_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
                updated_networks: {
                    seen: expected_data.seen,
                    sent: expected_data.sent,
                },
                removed_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
            },
        });
    }

    #[test_case(vec![Ok(())], SeenSentData { seen: 1, sent: 1 }; "success")]
    #[test_case(
        vec![
            Err(NetworkManagerError::Remove(fnp_socketproxy::NetworkRegistryRemoveError::NotFound)),
        ],
        SeenSentData { seen: 1, sent: 0 };
        "error"
    )]
    #[::fuchsia::test(threads = 2)]
    async fn test_remove_network_with_proxy(
        results: Vec<Result<(), NetworkManagerError>>,
        expected_data: SeenSentData,
    ) {
        let inspector = fuchsia_inspect::Inspector::default();
        let manager = &setup_proxy(inspector.root(), results).0;

        let network_id = 1;
        let network = test_network_message_from_id(network_id);

        // Insert the network manually and then remove it.
        {
            let mut inner_guard = manager.lock();
            let _ = inner_guard.as_mut().networks.insert(network_id, Some(network.clone()));
        }

        // `remove_network` returns Ok(()) as long as the network
        // removal is applied locally, regardless of if the call
        // is sent to the socketproxy successfully.
        assert_matches!(manager.remove_network(network_id), Ok(()));

        assert_data_tree!(inspector, root: {
            nmfs: {
                default_ids_set: {
                    seen: 0u64,
                    sent: 0u64,
                },
                added_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
                updated_networks: {
                    seen: 0u64,
                    sent: 0u64,
                },
                removed_networks: {
                    seen: expected_data.seen,
                    sent: expected_data.sent,
                },
            },
        });
    }

    #[::fuchsia::test(threads = 2)]
    async fn test_multiple_operations_with_proxy() {
        let inspector = fuchsia_inspect::Inspector::default();
        let results = vec![
            // Network added to Manager, but not added to socketproxy.
            Err(NetworkManagerError::Add(
                fnp_socketproxy::NetworkRegistryAddError::MissingNetworkId,
            )),
            // Network added to Manager and to socketproxy.
            Ok(()),
            // Network set as default in Manager, but not in socketproxy.
            Err(NetworkManagerError::SetDefault(
                fnp_socketproxy::NetworkRegistrySetDefaultError::NotFound,
            )),
            // Network set as default in Manager and in socketproxy.
            Ok(()),
            // Network updated in Manager, but not in socketproxy.
            Err(NetworkManagerError::Update(fnp_socketproxy::NetworkRegistryUpdateError::NotFound)),
            // Network updated in Manager and in socketproxy.
            Ok(()),
            // Unset the default network so the network is eligible to be removed.
            Ok(()),
            // Network removed from Manager, but not from socketproxy.
            Err(NetworkManagerError::Remove(fnp_socketproxy::NetworkRegistryRemoveError::NotFound)),
            // Network removed from Manager and from socketproxy.
            Ok(()),
        ];
        let manager = &setup_proxy(inspector.root(), results).0;

        // Add a network that doesn't get sent to the socketproxy.
        let network1 = test_network_message_from_id(1);
        assert_matches!(manager.add_network(network1.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Add a network that gets sent to the socketproxy.
        let network2 = test_network_message_from_id(2);
        assert_matches!(manager.add_network(network2.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                added_networks: {
                    seen: 2u64,
                    sent: 1u64,
                },
            },
        });

        // Set the default network that isn't known to the socketproxy.
        manager.set_default_network_id(Some(1));
        assert_eq!(manager.get_default_network_id(), Some(1));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Set the default network that is known to the socketproxy.
        manager.set_default_network_id(Some(2));
        assert_eq!(manager.get_default_network_id(), Some(2));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 2u64,
                    sent: 1u64,
                },
            },
        });

        // Update a network not known to the socketproxy.
        let mut network1_updated = network1.clone();
        network1_updated.mark = 1;
        assert_matches!(manager.update_network(network1_updated.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                updated_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Update a network that is known to the socketproxy.
        let mut network2_updated = network2.clone();
        network2_updated.mark = 2;
        assert_matches!(manager.update_network(network2_updated.clone()), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                updated_networks: {
                    seen: 2u64,
                    sent: 1u64,
                },
            },
        });

        // The default network must be unset first to remove this network.
        manager.set_default_network_id(None);
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                default_ids_set: {
                    seen: 3u64,
                    sent: 2u64,
                },
            },
        });

        // Remove a network that doesn't get sent to the socketproxy.
        assert_matches!(manager.remove_network(1), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: contains {
                removed_networks: {
                    seen: 1u64,
                    sent: 0u64,
                },
            },
        });

        // Remove a network that gets sent to the socketproxy.
        assert_matches!(manager.remove_network(2), Ok(()));
        assert_data_tree!(inspector, root: {
            nmfs: {
                default_ids_set: {
                    seen: 3u64,
                    sent: 2u64,
                },
                added_networks: {
                    seen: 2u64,
                    sent: 1u64,
                },
                updated_networks: {
                    seen: 2u64,
                    sent: 1u64,
                },
                removed_networks: {
                    seen: 2u64,
                    sent: 1u64,
                },
            },
        });
    }
}
