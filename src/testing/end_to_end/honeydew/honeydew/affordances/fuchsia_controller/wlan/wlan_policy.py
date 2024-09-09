# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""WLAN policy affordance implementation using Fuchsia Controller."""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass

import fidl.fuchsia_wlan_policy as f_wlan_policy
from fuchsia_controller_py import Channel, ZxStatus
from fuchsia_controller_py.wrappers import AsyncAdapter, asyncmethod

from honeydew import errors
from honeydew.interfaces.affordances.wlan import wlan_policy
from honeydew.interfaces.device_classes import affordances_capable
from honeydew.interfaces.transports import ffx as ffx_transport
from honeydew.interfaces.transports import fuchsia_controller as fc_transport
from honeydew.typing.custom_types import FidlEndpoint
from honeydew.typing.wlan import (
    ClientStateSummary,
    NetworkConfig,
    RequestStatus,
    SecurityType,
)

# List of required FIDLs for the WLAN Fuchsia Controller affordance.
_REQUIRED_CAPABILITIES = [
    "fuchsia.wlan.policy.ClientListener",
    "fuchsia.wlan.policy.ClientProvider",
    "fuchsia.wlan.phyimpl",
]

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Fuchsia Controller proxies
_CLIENT_PROVIDER_PROXY = FidlEndpoint(
    "core/wlancfg", "fuchsia.wlan.policy.ClientProvider"
)
_CLIENT_LISTENER_PROXY = FidlEndpoint(
    "core/wlancfg", "fuchsia.wlan.policy.ClientListener"
)

# Length of a pre-shared key (PSK) used as a password.
_PSK_LENGTH = 64


def _parse_password(password: str | None) -> f_wlan_policy.Credential:
    """Parse a password into a Credential.

    Args:
        password: String password, pre-shared key in hex form with length 64, or
            None/empty to represent open.

    Return:
        A fuchsia.wlan.policy/Credential union object.
    """
    credential = f_wlan_policy.Credential()

    if not password:
        credential.none = f_wlan_policy.Empty()
    elif len(password) == _PSK_LENGTH:
        credential.psk = list(bytes.fromhex(password))
    else:
        credential.password = list(password.encode("utf-8"))

    return credential


@dataclass
class ClientControllerState:
    proxy: f_wlan_policy.ClientController.Client
    updates: asyncio.Queue[ClientStateSummary]
    # Keep the async task for fuchsia.wlan.policy/ClientStateUpdates so it
    # doesn't get garbage collected then cancelled.
    client_state_updates_server_task: asyncio.Task[None]


class WlanPolicy(AsyncAdapter, wlan_policy.WlanPolicy):
    """WLAN affordance implemented with Fuchsia Controller."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        """Create a WLAN Policy Fuchsia Controller affordance.

        Args:
            device_name: Device name returned by `ffx target list`.
            ffx: FFX transport.
            fuchsia_controller: Fuchsia Controller transport.
            reboot_affordance: Object that implements RebootCapableDevice.
        """
        super().__init__()
        self._verify_supported(device_name, ffx)

        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self._client_controller: ClientControllerState | None = None

        self._connect_proxy()
        self._reboot_affordance.register_for_on_device_boot(self._connect_proxy)

    def close(self) -> None:
        """Release handle on client controller.

        This needs to be called on test class teardown otherwise the device may
        be left in an inoperable state where no other components or tests can
        access state-changing WLAN Policy APIs.

        This is idempotent and irreversible. No other methods should be called
        after this one.
        """
        if self._client_controller:
            self._cancel_task(
                self._client_controller.client_state_updates_server_task
            )
            self._client_controller = None

        if not self.loop().is_closed():
            self.loop().stop()
            self.loop().run_forever()  # Handle pending tasks
            self.loop().close()

    def _cancel_task(self, task: asyncio.Task[None]) -> None:
        """Cancel a task then verify it has been cancelled.

        Args:
            task: The task to cancel

        Raises:
            RuntimeError: failed cancel verification
        """
        if not task.cancel():
            # Task was already done or cancelled, nothing else to do.
            return

        # Wait for task to completely cancel.
        try:
            self.loop().run_until_complete(task)
            raise RuntimeError(
                "Expected cancellation of task to raise CancelledError"
            )
        except asyncio.exceptions.CancelledError:
            pass  # expected

    def _verify_supported(self, device: str, ffx: ffx_transport.FFX) -> None:
        """Check if WLAN Policy is supported on the DUT.

        Args:
            device: Device name returned by `ffx target list`.
            ffx: FFX transport

        Raises:
            NotSupportedError: A required component capability is not available.
        """
        for capability in _REQUIRED_CAPABILITIES:
            # TODO(http://b/359342196): This is a maintenance burden; find a
            # better way to detect FIDL component capabilities.
            if capability not in ffx.run(
                ["component", "capability", capability]
            ):
                _LOGGER.warning(
                    "All available WLAN component capabilities:\n%s",
                    ffx.run(["component", "capability", "fuchsia.wlan"]),
                )
                raise errors.NotSupportedError(
                    f'Component capability "{capability}" not exposed by device '
                    f"{device}; this build of Fuchsia does not support the "
                    "WLAN FC affordance."
                )

    def _connect_proxy(self) -> None:
        """Re-initializes connection to the WLAN stack."""
        self._client_provider_proxy = f_wlan_policy.ClientProvider.Client(
            self._fc_transport.connect_device_proxy(_CLIENT_PROVIDER_PROXY)
        )

    def connect(
        self, target_ssid: str, security_type: SecurityType
    ) -> RequestStatus:
        """Triggers connection to a network.

        Args:
            target_ssid: The network to connect to. Must have been previously
                saved in order for a successful connection to happen.
            security_type: The security protocol of the network.

        Returns:
            A RequestStatus response to the connect request

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            TypeError: Return value not a string.
        """
        raise NotImplementedError()

    def create_client_controller(self) -> None:
        """Initializes the client controller.

        See fuchsia.wlan.policy/ClientProvider.GetController().

        Raises:
            HoneydewWlanError: Error from WLAN stack.
        """
        if self._client_controller:
            self._cancel_task(
                self._client_controller.client_state_updates_server_task
            )
            self._client_controller = None

        controller_client, controller_server = Channel.create()
        client_controller_proxy = f_wlan_policy.ClientController.Client(
            controller_client.take()
        )

        updates: asyncio.Queue[ClientStateSummary] = asyncio.Queue()

        updates_client, updates_server = Channel.create()
        client_state_updates_server = ClientStateUpdatesImpl(
            updates_server, updates
        )
        task = self.loop().create_task(client_state_updates_server.serve())

        try:
            self._client_provider_proxy.get_controller(
                requests=controller_server.take(),
                updates=updates_client.take(),
            )
        except ZxStatus as status:
            raise errors.HoneydewWlanError(
                f"ClientProvider.GetController() error {status}"
            ) from status

        self._client_controller = ClientControllerState(
            proxy=client_controller_proxy,
            updates=updates,
            client_state_updates_server_task=task,
        )

    def get_saved_networks(self) -> list[NetworkConfig]:
        """Gets networks saved on device.

        Returns:
            A list of NetworkConfigs.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            TypeError: Return values not correct types.
        """
        raise NotImplementedError()

    @asyncmethod
    # pylint: disable-next=invalid-overridden-method
    async def get_update(
        self,
        timeout: float | None = None,
    ) -> ClientStateSummary:
        """Gets one client listener update.

        This call will return with an update immediately the
        first time the update listener is initialized by setting a new listener
        or by creating a client controller before setting a new listener.
        Subsequent calls will hang until there is a change since the last
        update call.

        Args:
            timeout: Timeout in seconds to wait for the get_update command to
                return. By default it is set to None (which means timeout is
                disabled)

        Returns:
            An update of connection status. If there is no error, the result is
            a WlanPolicyUpdate with a structure that matches the FIDL
            ClientStateSummary struct given for updates.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            TypeError: Return values not correct types.
        """
        if self._client_controller is None:
            self.create_client_controller()
        assert self._client_controller is not None

        return await self._client_controller.updates.get()

    def remove_all_networks(self) -> None:
        """Deletes all saved networks on the device.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
        """
        raise NotImplementedError()

    def remove_network(
        self,
        target_ssid: str,
        security_type: SecurityType,
        target_pwd: str | None = None,
    ) -> None:
        """Removes or "forgets" a network from saved networks.

        Args:
            target_ssid: The network to remove.
            security_type: The security protocol of the network.
            target_pwd: The credential being saved with the network. No password
                is equivalent to an empty string.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
        """
        raise NotImplementedError()

    @asyncmethod
    # pylint: disable-next=invalid-overridden-method
    async def save_network(
        self,
        target_ssid: str,
        security_type: SecurityType,
        target_pwd: str | None = None,
    ) -> None:
        """Saves a network to the device.

        Args:
            target_ssid: The network to save.
            security_type: The security protocol of the network.
            target_pwd: The credential being saved with the network. No password
                is equivalent to an empty string.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            RuntimeError: A client controller has not been created yet
        """
        if self._client_controller is None:
            raise RuntimeError(
                "Client controller has not been initialized; call "
                "create_client_controller() before save_network()"
            )

        try:
            res = await self._client_controller.proxy.save_network(
                config=f_wlan_policy.NetworkConfig(
                    id=f_wlan_policy.NetworkIdentifier(
                        ssid=list(target_ssid.encode("utf-8")),
                        type=security_type.to_fidl(),
                    ),
                    credential=_parse_password(target_pwd),
                ),
            )
            if res.err:
                raise errors.HoneydewWlanError(
                    "ClientController.SaveNetworks() NetworkConfigChangeError "
                    f"{res.err.name}"
                )
        except ZxStatus as status:
            raise errors.HoneydewWlanError(
                f"ClientController.SaveNetwork() error {status}"
            ) from status

    def scan_for_networks(self) -> list[str]:
        """Scans for networks.

        Returns:
            A list of network SSIDs that can be connected to.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            TypeError: Return value not a list.
        """
        raise NotImplementedError()

    def set_new_update_listener(self) -> None:
        """Sets the update listener stream of the facade to a new stream.

        This causes updates to be reset. Intended to be used between tests so
        that the behavior of updates in a test is independent from previous
        tests.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
        """
        if self._client_controller is None:
            # There is no running fuchsia.wlan.policy/ClientStateUpdates server.
            # Creating one is equivalent to creating a new update listener.
            self.create_client_controller()
            return

        # Replace the existing ClientStateUpdates server without giving up our
        # handle to ClientController. This is necessary since the ClientProvider
        # API is designed to only allow a single caller to make ClientController
        # calls which would impact WLAN state. If we lose our handle to the
        # ClientController, some other component on the system could take it.
        self._cancel_task(
            self._client_controller.client_state_updates_server_task
        )

        client_listener_proxy = f_wlan_policy.ClientListener.Client(
            self._fc_transport.connect_device_proxy(_CLIENT_LISTENER_PROXY)
        )

        updates: asyncio.Queue[ClientStateSummary] = asyncio.Queue()
        updates_client, updates_server = Channel.create()
        client_state_updates_server = ClientStateUpdatesImpl(
            updates_server, updates
        )
        task = self._async_adapter_loop.create_task(
            client_state_updates_server.serve()
        )

        try:
            client_listener_proxy.get_listener(
                updates=updates_client.take(),
            )
        except ZxStatus as status:
            raise errors.HoneydewWlanError(
                f"ClientListener.GetListener() error {status}"
            ) from status

        self._client_controller.updates = updates
        self._client_controller.client_state_updates_server_task = task

    @asyncmethod
    # pylint: disable-next=invalid-overridden-method
    async def start_client_connections(self) -> None:
        """Enables device to initiate connections to networks.

        See fuchsia.wlan.policy/ClientController.StartClientConnections().

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            RuntimeError: A client controller has not been created yet
        """
        if self._client_controller is None:
            raise RuntimeError(
                "Client controller has not been initialized; call "
                "create_client_controller() before start_client_connections()"
            )

        try:
            resp = (
                await self._client_controller.proxy.start_client_connections()
            )
            status = RequestStatus.from_fidl(resp.status)
            if status != RequestStatus.ACKNOWLEDGED:
                raise errors.HoneydewWlanError(
                    "ClientController.StartClientConnections() returned "
                    f"request status {status}"
                )
        except ZxStatus as status:
            raise errors.HoneydewWlanError(
                f"ClientController.StartClientConnections() error {status}"
            ) from status

    @asyncmethod
    # pylint: disable-next=invalid-overridden-method
    async def stop_client_connections(self) -> None:
        """Disables device for initiating connections to networks.

        See fuchsia.wlan.policy/ClientController.StopClientConnections().

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            RuntimeError: A client controller has not been created yet
        """
        if self._client_controller is None:
            raise RuntimeError(
                "Client controller has not been initialized; call "
                "create_client_controller() before stop_client_connections()"
            )

        try:
            resp = await self._client_controller.proxy.stop_client_connections()
            status = RequestStatus.from_fidl(resp.status)
            if status != RequestStatus.ACKNOWLEDGED:
                raise errors.HoneydewWlanError(
                    "ClientController.StopClientConnections() returned "
                    f"request status {status}"
                )
        except ZxStatus as status:
            raise errors.HoneydewWlanError(
                f"ClientController.StopClientConnections() error {status}"
            ) from status


class ClientStateUpdatesImpl(f_wlan_policy.ClientStateUpdates.Server):
    """Server to receive WLAN status changes.

    Receives updates for client connections and the associated network state
    These updates contain information about whether or not the device will
    attempt to connect to networks, saved network configuration change
    information, individual connection state information by NetworkIdentifier
    and connection attempt information.
    """

    def __init__(
        self, server: Channel, updates: asyncio.Queue[ClientStateSummary]
    ) -> None:
        super().__init__(server)
        self._updates = updates
        _LOGGER.debug("Started ClientStateUpdates server")

    async def on_client_state_update(
        self,
        request: f_wlan_policy.ClientStateUpdatesOnClientStateUpdateRequest,
    ) -> None:
        """Detected a change to the state or registered listeners.

        Args:
            request: Current summary of WLAN client state.
        """
        summary = ClientStateSummary.from_fidl(request.summary)
        _LOGGER.debug("OnClientStateUpdate called with %s", repr(summary))
        await self._updates.put(summary)
