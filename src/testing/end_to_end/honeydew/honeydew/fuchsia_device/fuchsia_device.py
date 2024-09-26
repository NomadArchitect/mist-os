# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""FuchsiaDevice abstract base class implementation using Fuchsia-Controller."""

import asyncio
import ipaddress
import logging
import os
from collections.abc import Callable
from datetime import datetime
from typing import Any

import fidl.fuchsia_buildinfo as f_buildinfo
import fidl.fuchsia_developer_remotecontrol as fd_remotecontrol
import fidl.fuchsia_diagnostics as f_diagnostics
import fidl.fuchsia_feedback as f_feedback
import fidl.fuchsia_hardware_power_statecontrol as fhp_statecontrol
import fidl.fuchsia_hwinfo as f_hwinfo
import fidl.fuchsia_io as f_io
import fuchsia_controller_py as fcp

from honeydew import errors
from honeydew.affordances.ffx import inspect as inspect_ffx
from honeydew.affordances.ffx import session as session_ffx
from honeydew.affordances.ffx.ui import screenshot as screenshot_ffx
from honeydew.affordances.fuchsia_controller import netstack as netstack_fc
from honeydew.affordances.fuchsia_controller import rtc as rtc_fc
from honeydew.affordances.fuchsia_controller import tracing as tracing_fc
from honeydew.affordances.fuchsia_controller.bluetooth.profiles import (
    bluetooth_gap as gap_fc,
)
from honeydew.affordances.fuchsia_controller.bluetooth.profiles import (
    bluetooth_le as le_fc,
)
from honeydew.affordances.fuchsia_controller.ui import (
    user_input as user_input_fc,
)
from honeydew.affordances.fuchsia_controller.wlan import wlan as wlan_fc
from honeydew.affordances.fuchsia_controller.wlan import (
    wlan_policy as wlan_policy_fc,
)
from honeydew.affordances.sl4f.bluetooth.profiles import (
    bluetooth_avrcp as bluetooth_avrcp_sl4f,
)
from honeydew.affordances.sl4f.bluetooth.profiles import (
    bluetooth_gap as bluetooth_gap_sl4f,
)
from honeydew.affordances.sl4f.wlan import wlan as wlan_sl4f
from honeydew.affordances.sl4f.wlan import wlan_policy as wlan_policy_sl4f
from honeydew.affordances.starnix import (
    system_power_state_controller as system_power_state_controller_starnix,
)
from honeydew.interfaces.affordances import inspect as inspect_interface
from honeydew.interfaces.affordances import (
    rtc,
    session,
    system_power_state_controller,
    tracing,
)
from honeydew.interfaces.affordances.bluetooth.profiles import (
    bluetooth_avrcp as bluetooth_avrcp_interface,
)
from honeydew.interfaces.affordances.bluetooth.profiles import (
    bluetooth_gap as bluetooth_gap_interface,
)
from honeydew.interfaces.affordances.bluetooth.profiles import (
    bluetooth_le as bluetooth_le_interface,
)
from honeydew.interfaces.affordances.ui import screenshot, user_input
from honeydew.interfaces.affordances.wlan import wlan, wlan_policy
from honeydew.interfaces.auxiliary_devices import (
    power_switch as power_switch_interface,
)
from honeydew.interfaces.device_classes import affordances_capable
from honeydew.interfaces.device_classes import (
    fuchsia_device as fuchsia_device_interface,
)
from honeydew.interfaces.transports import (
    fastboot as fastboot_transport_interface,
)
from honeydew.interfaces.transports import ffx as ffx_transport_interface
from honeydew.interfaces.transports import (
    fuchsia_controller as fuchsia_controller_transport_interface,
)
from honeydew.interfaces.transports import serial as serial_transport_interface
from honeydew.interfaces.transports import sl4f as sl4f_transport_interface
from honeydew.transports import fastboot as fastboot_transport
from honeydew.transports import ffx as ffx_transport
from honeydew.transports import (
    fuchsia_controller as fuchsia_controller_transport,
)
from honeydew.transports import serial_using_unix_socket
from honeydew.transports import sl4f as sl4f_transport
from honeydew.typing import custom_types
from honeydew.utils import common, properties

_FC_PROXIES: dict[str, custom_types.FidlEndpoint] = {
    "BuildInfo": custom_types.FidlEndpoint(
        "/core/build-info", "fuchsia.buildinfo.Provider"
    ),
    "DeviceInfo": custom_types.FidlEndpoint(
        "/core/hwinfo", "fuchsia.hwinfo.Device"
    ),
    "Feedback": custom_types.FidlEndpoint(
        "/core/feedback", "fuchsia.feedback.DataProvider"
    ),
    "ProductInfo": custom_types.FidlEndpoint(
        "/core/hwinfo", "fuchsia.hwinfo.Product"
    ),
    "PowerAdmin": custom_types.FidlEndpoint(
        "/bootstrap/shutdown_shim", "fuchsia.hardware.power.statecontrol.Admin"
    ),
    "RemoteControl": custom_types.FidlEndpoint(
        "/core/remote-control", "fuchsia.developer.remotecontrol.RemoteControl"
    ),
}

_LOG_SEVERITIES: dict[custom_types.LEVEL, f_diagnostics.Severity] = {
    custom_types.LEVEL.INFO: f_diagnostics.Severity.INFO,
    custom_types.LEVEL.WARNING: f_diagnostics.Severity.WARN,
    custom_types.LEVEL.ERROR: f_diagnostics.Severity.ERROR,
}

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FuchsiaDevice(
    fuchsia_device_interface.FuchsiaDevice,
    affordances_capable.RebootCapableDevice,
    affordances_capable.FuchsiaDeviceLogger,
    affordances_capable.FuchsiaDeviceClose,
):
    """FuchsiaDevice abstract base class implementation using
    Fuchsia-Controller.

    Args:
        device_info: Fuchsia device information.
        ffx_config: Config that need to be used while running FFX commands.
        config: Honeydew device configuration, if any.
            Format:
                {
                    "transports": {
                        <transport_name>: {
                            <key>: <value>,
                            ...
                        },
                        ...
                    },
                    "affordances": {
                        <affordance_name>: {
                            <key>: <value>,
                            ...
                        },
                        ...
                    },
                }
            Example:
                {
                    "transports": {
                        "fuchsia_controller": {
                            "timeout": 30,
                        }
                    },
                    "affordances": {
                        "bluetooth": {
                            "implementation": "fuchsia-controller",
                        },
                        "wlan": {
                            "implementation": "sl4f",
                        }
                    },
                }

    Raises:
        errors.FFXCommandError: if FFX connection check fails.
        errors.FuchsiaControllerError: if FC connection check fails.
    """

    def __init__(
        self,
        device_info: custom_types.DeviceInfo,
        ffx_config: custom_types.FFXConfig,
        transport: custom_types.TRANSPORT,
        # intentionally made this a Dict instead of dataclass to minimize the changes in remaining Lacewing stack every time we need to add a new configuration item
        config: dict[str, Any] | None = None,
    ) -> None:
        _LOGGER.debug("Initializing Fuchsia-Controller based FuchsiaDevice")

        self._device_info: custom_types.DeviceInfo = device_info

        self._ffx_config: custom_types.FFXConfig = ffx_config

        self._transport: custom_types.TRANSPORT = transport

        self._on_device_boot_fns: list[Callable[[], None]] = []
        self._on_device_close_fns: list[Callable[[], None]] = []

        self._config: dict[str, Any] | None = config

        self.health_check()

        _LOGGER.debug("Initialized Fuchsia-Controller based FuchsiaDevice")

    # List all the persistent properties
    @properties.PersistentProperty
    def board(self) -> str:
        """Returns the board value of the device.

        Returns:
            board value of the device.

        Raises:
            errors.FfxCommandError: On failure.
        """
        return self.ffx.get_target_board()

    @properties.PersistentProperty
    def device_name(self) -> str:
        """Returns the name of the device.

        Returns:
            Name of the device.
        """
        return self._device_info.name

    @properties.PersistentProperty
    def manufacturer(self) -> str:
        """Returns the manufacturer of the device.

        Returns:
            Manufacturer of device.

        Raises:
            errors.FuchsiaDeviceError: On failure.
        """
        return self._product_info["manufacturer"]

    @properties.PersistentProperty
    def model(self) -> str:
        """Returns the model of the device.

        Returns:
            Model of device.

        Raises:
            errors.FuchsiaDeviceError: On failure.
        """
        return self._product_info["model"]

    @properties.PersistentProperty
    def product(self) -> str:
        """Returns the product value of the device.

        Returns:
            product value of the device.

        Raises:
            errors.FfxCommandError: On failure.
        """
        return self.ffx.get_target_product()

    @properties.PersistentProperty
    def product_name(self) -> str:
        """Returns the product name of the device.

        Returns:
            Product name of the device.

        Raises:
            errors.FuchsiaDeviceError: On failure.
        """
        return self._product_info["name"]

    @properties.PersistentProperty
    def serial_number(self) -> str:
        """Returns the serial number of the device.

        Returns:
            Serial number of the device.
        """
        return self._device_info_from_fidl["serial_number"]

    # List all the dynamic properties
    @properties.DynamicProperty
    def firmware_version(self) -> str:
        """Returns the firmware version of the device.

        Returns:
            Firmware version of the device.
        """
        return self._build_info["version"]

    # List all transports
    @properties.Transport
    def ffx(self) -> ffx_transport_interface.FFX:
        """Returns the FFX transport object.

        Returns:
            FFX transport interface implementation.

        Raises:
            errors.FfxCommandError: Failed to instantiate.
        """
        ffx_obj: ffx_transport_interface.FFX = ffx_transport.FFX(
            target_name=self.device_name,
            config=self._ffx_config,
            target_ip_port=self._device_info.ip_port,
        )
        return ffx_obj

    @properties.Transport
    def fuchsia_controller(
        self,
    ) -> fuchsia_controller_transport_interface.FuchsiaController:
        """Returns the Fuchsia-Controller transport object.

        Returns:
            Fuchsia-Controller transport interface implementation.

        Raises:
            errors.FuchsiaControllerError: Failed to instantiate.
        """
        fuchsia_controller_obj: (
            fuchsia_controller_transport_interface.FuchsiaController
        ) = fuchsia_controller_transport.FuchsiaController(
            target_name=self.device_name,
            config=self._ffx_config,
            target_ip_port=self._device_info.ip_port,
        )
        return fuchsia_controller_obj

    @properties.Transport
    def fastboot(self) -> fastboot_transport_interface.Fastboot:
        """Returns the Fastboot transport object.

        Returns:
            Fastboot transport interface implementation.

        Raises:
            errors.FuchsiaDeviceError: Failed to instantiate.
        """
        fastboot_obj: fastboot_transport_interface.Fastboot = (
            fastboot_transport.Fastboot(
                device_name=self.device_name,
                reboot_affordance=self,
                ffx_transport=self.ffx,
            )
        )
        return fastboot_obj

    @properties.Transport
    def serial(self) -> serial_transport_interface.Serial:
        """Returns the Serial transport object.

        Returns:
            Serial transport object.
        """
        if self._device_info.serial_socket is None:
            raise errors.FuchsiaDeviceError(
                "'device_serial_socket' arg need to be provided during the init to use Serial affordance"
            )

        serial_obj: serial_transport_interface.Serial = (
            serial_using_unix_socket.Serial(
                device_name=self.device_name,
                socket_path=self._device_info.serial_socket,
            )
        )
        return serial_obj

    @properties.Transport
    def sl4f(self) -> sl4f_transport_interface.SL4F:
        """Returns the SL4F transport object.

        Returns:
            SL4F transport interface implementation.

        Raises:
            errors.Sl4fError: Failed to instantiate.
        """
        device_ip: ipaddress.IPv4Address | ipaddress.IPv6Address | None = None
        if self._device_info.ip_port:
            device_ip = self._device_info.ip_port.ip

        sl4f_obj: sl4f_transport_interface.SL4F = sl4f_transport.SL4F(
            device_name=self.device_name,
            device_ip=device_ip,
            ffx_transport=self.ffx,
        )
        return sl4f_obj

    # List all the affordances
    @properties.Affordance
    def session(self) -> session.Session:
        """Returns a session affordance object.

        Returns:
            session.Session object
        """
        return session_ffx.Session(device_name=self.device_name, ffx=self.ffx)

    @properties.Affordance
    def screenshot(self) -> screenshot.Screenshot:
        """Returns a screenshot affordance object.

        Returns:
            screenshot.Screenshot object
        """
        return screenshot_ffx.Screenshot(self.ffx)

    @properties.Affordance
    def system_power_state_controller(
        self,
    ) -> system_power_state_controller.SystemPowerStateController:
        """Returns a SystemPowerStateController affordance object.

        Returns:
            system_power_state_controller.SystemPowerStateController object

        Raises:
            errors.NotSupportedError: If Fuchsia device does not support Starnix
        """
        return system_power_state_controller_starnix.SystemPowerStateController(
            device_name=self.device_name,
            ffx=self.ffx,
            inspect=self.inspect,
            device_logger=self,
        )

    @properties.Affordance
    def rtc(self) -> rtc.Rtc:
        """Returns an Rtc affordance object.

        Returns:
            rtc.Rtc object
        """
        return rtc_fc.Rtc(
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def tracing(self) -> tracing.Tracing:
        """Returns a tracing affordance object.

        Returns:
            tracing.Tracing object
        """
        return tracing_fc.Tracing(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def user_input(self) -> user_input.UserInput:
        """Returns an user input affordance object.

        Returns:
            user_input.UserInput object
        """
        return user_input_fc.UserInput(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            ffx_transport=self.ffx,
        )

    @properties.Affordance
    def bluetooth_avrcp(self) -> bluetooth_avrcp_interface.BluetoothAvrcp:
        """Returns a BluetoothAvrcp affordance object.

        Returns:
            bluetooth_avrcp.BluetoothAvrcp object
        """
        if (
            self._transport
            == custom_types.TRANSPORT.FUCHSIA_CONTROLLER_PREFERRED
        ):
            return bluetooth_avrcp_sl4f.BluetoothAvrcp(
                device_name=self.device_name,
                sl4f=self.sl4f,
                reboot_affordance=self,
            )
        raise NotImplementedError

    @properties.Affordance
    def bluetooth_le(self) -> bluetooth_le_interface.BluetoothLE:
        """Returns a BluetoothGap affordance object.

        Returns:
            bluetooth_gap.BluetoothGap object
        """
        return le_fc.BluetoothLE(
            device_name=self.device_name,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    @properties.Affordance
    def bluetooth_gap(self) -> bluetooth_gap_interface.BluetoothGap:
        """Returns a BluetoothGap affordance object.

        Returns:
            bluetooth_gap.BluetoothGap object
        """
        if (
            self._transport
            == custom_types.TRANSPORT.FUCHSIA_CONTROLLER_PREFERRED
        ):
            return bluetooth_gap_sl4f.BluetoothGap(
                device_name=self.device_name,
                sl4f=self.sl4f,
                reboot_affordance=self,
            )
        else:
            return gap_fc.BluetoothGap(
                device_name=self.device_name,
                fuchsia_controller=self.fuchsia_controller,
                reboot_affordance=self,
            )

    @properties.Affordance
    def wlan_policy(self) -> wlan_policy.WlanPolicy:
        """Returns a wlan_policy affordance object.

        Returns:
            wlan_policy.WlanPolicy object
        """
        if (
            self._transport
            == custom_types.TRANSPORT.FUCHSIA_CONTROLLER_PREFERRED
        ):
            return wlan_policy_sl4f.WlanPolicy(
                device_name=self.device_name, sl4f=self.sl4f
            )
        else:
            return wlan_policy_fc.WlanPolicy(
                device_name=self.device_name,
                ffx=self.ffx,
                fuchsia_controller=self.fuchsia_controller,
                reboot_affordance=self,
                fuchsia_device_close=self,
            )

    @properties.Affordance
    def wlan(self) -> wlan.Wlan:
        """Returns a wlan affordance object.

        Returns:
            wlan.Wlan object
        """
        if (
            self._transport
            == custom_types.TRANSPORT.FUCHSIA_CONTROLLER_PREFERRED
        ):
            return wlan_sl4f.Wlan(device_name=self.device_name, sl4f=self.sl4f)
        else:
            return wlan_fc.Wlan(
                device_name=self.device_name,
                ffx=self.ffx,
                fuchsia_controller=self.fuchsia_controller,
                reboot_affordance=self,
            )

    @properties.Affordance
    def inspect(self) -> inspect_interface.Inspect:
        """Returns a inspect affordance object.

        Returns:
            inspect.Inspect object
        """
        return inspect_ffx.Inspect(
            device_name=self.device_name,
            ffx=self.ffx,
        )

    @properties.Affordance
    def netstack(self) -> netstack_fc.Netstack:
        """Returns a netstack affordance object.

        Returns:
            netstack.Netstack object
        """
        return netstack_fc.Netstack(
            device_name=self.device_name,
            ffx=self.ffx,
            fuchsia_controller=self.fuchsia_controller,
            reboot_affordance=self,
        )

    # List all the public methods
    def close(self) -> None:
        """Clean up method."""
        for on_device_close_fns in self._on_device_close_fns:
            _LOGGER.info("Calling %s", on_device_close_fns.__qualname__)
            on_device_close_fns()

    def health_check(self) -> None:
        """Ensure device is healthy.

        Raises:
            errors.FfxConnectionError
            errors.FuchsiaControllerConnectionError
        """
        # Note - FFX need to be invoked first before FC as FC depends on the daemon that will be created by FFX
        self.ffx.check_connection()
        self.fuchsia_controller.check_connection()
        if (
            self._transport
            == custom_types.TRANSPORT.FUCHSIA_CONTROLLER_PREFERRED
        ):
            self.sl4f.check_connection()

    def log_message_to_device(
        self, message: str, level: custom_types.LEVEL
    ) -> None:
        """Log message to fuchsia device at specified level.

        Args:
            message: Message that need to logged.
            level: Log message level.

        Raises:
            errors.FuchsiaControllerError: On communications failure.
            errors.Sl4FError: On communications failure.
        """
        timestamp: str = datetime.now().strftime("%Y-%m-%d-%I-%M-%S-%p")
        message = f"[Host Time: {timestamp}] - {message}"
        self._send_log_command(tag="lacewing", message=message, level=level)

    def on_device_boot(self) -> None:
        """Take actions after the device is rebooted.

        Raises:
            errors.FuchsiaControllerError: On communications failure.
            errors.Sl4FError: On communications failure.
        """
        # Restart the SL4F server on device boot up.
        if (
            self._transport
            == custom_types.TRANSPORT.FUCHSIA_CONTROLLER_PREFERRED
        ):
            common.retry(fn=self.sl4f.start_server, wait_time=5)

        # Create a new Fuchsia controller context for new device connection.
        self.fuchsia_controller.create_context()

        # Ensure device is healthy
        self.health_check()

        for on_device_boot_fn in self._on_device_boot_fns:
            _LOGGER.info("Calling %s", on_device_boot_fn.__qualname__)
            on_device_boot_fn()

    def power_cycle(
        self,
        power_switch: power_switch_interface.PowerSwitch,
        outlet: int | None = None,
    ) -> None:
        """Power cycle (power off, wait for delay, power on) the device.

        Args:
            power_switch: Implementation of PowerSwitch interface.
            outlet (int): If required by power switch hardware, outlet on
                power switch hardware where this fuchsia device is connected.

        Raises:
            errors.FuchsiaControllerError: On communications failure.
            errors.Sl4FError: On communications failure.
        """
        _LOGGER.info("Power cycling %s...", self.device_name)

        try:
            self.log_message_to_device(
                message=f"Powering cycling {self.device_name}...",
                level=custom_types.LEVEL.INFO,
            )
        except Exception:  # pylint: disable=broad-except
            # power_cycle can be used as a recovery mechanism when device is
            # unhealthy. So any calls to device prior to power_cycle can
            # fail in such cases and thus ignore them.
            pass

        _LOGGER.info("Powering off %s...", self.device_name)
        power_switch.power_off(outlet)
        self.wait_for_offline()

        _LOGGER.info("Powering on %s...", self.device_name)
        power_switch.power_on(outlet)
        self.wait_for_online()

        self.on_device_boot()

        self.log_message_to_device(
            message=f"Successfully power cycled {self.device_name}...",
            level=custom_types.LEVEL.INFO,
        )

    def reboot(self) -> None:
        """Soft reboot the device.

        Raises:
            errors.FuchsiaControllerError: On communications failure.
            errors.Sl4FError: On communications failure.
        """
        _LOGGER.info("Lacewing is rebooting %s...", self.device_name)
        self.log_message_to_device(
            message=f"Rebooting {self.device_name}...",
            level=custom_types.LEVEL.INFO,
        )

        self._send_reboot_command()

        self.wait_for_offline()
        self.wait_for_online()
        self.on_device_boot()

        self.log_message_to_device(
            message=f"Successfully rebooted {self.device_name}...",
            level=custom_types.LEVEL.INFO,
        )

    def register_for_on_device_boot(self, fn: Callable[[], None]) -> None:
        """Register a function that will be called in `on_device_boot()`.

        Args:
            fn: Function that need to be called after FuchsiaDevice boot up.
        """
        self._on_device_boot_fns.append(fn)

    def register_for_on_device_close(self, fn: Callable[[], None]) -> None:
        """Register a function that will be called during device clean up in `close()`.

        Args:
            fn: Function that need to be called during FuchsiaDevice cleanup.
        """
        self._on_device_close_fns.append(fn)

    def snapshot(self, directory: str, snapshot_file: str | None = None) -> str:
        """Captures the snapshot of the device.

        Args:
            directory: Absolute path on the host where snapshot file will be
                saved. If this directory does not exist, this method will create
                it.

            snapshot_file: Name of the output snapshot file.
                If not provided, API will create a name using
                "Snapshot_{device_name}_{'%Y-%m-%d-%I-%M-%S-%p'}" format.

        Returns:
            Absolute path of the snapshot file.

        Raises:
            errors.FuchsiaControllerError: On communications failure.
            errors.Sl4FError: On communications failure.
        """
        _LOGGER.info("Collecting snapshot on %s...", self.device_name)
        # Take the snapshot before creating the directory or file, as
        # _send_snapshot_command may raise an exception.
        snapshot_bytes: bytes = self._send_snapshot_command()

        directory = os.path.abspath(directory)
        try:
            os.makedirs(directory)
        except FileExistsError:
            pass

        if not snapshot_file:
            timestamp: str = datetime.now().strftime("%Y-%m-%d-%I-%M-%S-%p")
            snapshot_file = f"Snapshot_{self.device_name}_{timestamp}.zip"
        snapshot_file_path: str = os.path.join(directory, snapshot_file)

        with open(snapshot_file_path, "wb") as snapshot_binary_zip:
            snapshot_binary_zip.write(snapshot_bytes)

        _LOGGER.info("Snapshot file has been saved @ '%s'", snapshot_file_path)
        return snapshot_file_path

    def wait_for_offline(self) -> None:
        """Wait for Fuchsia device to go offline.

        Raises:
            errors.FuchsiaDeviceError: If device is not offline.
        """
        _LOGGER.info("Waiting for %s to go offline...", self.device_name)
        try:
            self.ffx.wait_for_rcs_disconnection()
            _LOGGER.info("%s is offline.", self.device_name)
        except (errors.DeviceNotConnectedError, errors.FfxCommandError) as err:
            raise errors.FuchsiaDeviceError(
                f"'{self.device_name}' failed to go offline."
            ) from err

    def wait_for_online(self) -> None:
        """Wait for Fuchsia device to go online.

        Raises:
            errors.FuchsiaDeviceError: If device is not online.
        """
        _LOGGER.info("Waiting for %s to go online...", self.device_name)
        try:
            self.ffx.wait_for_rcs_connection()
            _LOGGER.info("%s is online.", self.device_name)
        except (errors.DeviceNotConnectedError, errors.FfxCommandError) as err:
            raise errors.FuchsiaDeviceError(
                f"'{self.device_name}' failed to go online."
            ) from err

    # List all private properties
    @property
    def _build_info(self) -> dict[str, Any]:
        """Returns the build information of the device.

        Returns:
            Build info dict.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            buildinfo_provider_proxy = f_buildinfo.Provider.Client(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["BuildInfo"]
                )
            )
            build_info_resp = asyncio.run(
                buildinfo_provider_proxy.get_build_info()
            )
            return build_info_resp.build_info
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    @property
    def _device_info_from_fidl(self) -> dict[str, Any]:
        """Returns the device information of the device.

        Returns:
            Device info dict.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            hwinfo_device_proxy = f_hwinfo.Device.Client(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["DeviceInfo"]
                )
            )
            device_info_resp = asyncio.run(hwinfo_device_proxy.get_info())
            return device_info_resp.info
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    @property
    def _product_info(self) -> dict[str, Any]:
        """Returns the product information of the device.

        Returns:
            Product info dict.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            hwinfo_product_proxy = f_hwinfo.Product.Client(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["ProductInfo"]
                )
            )
            product_info_resp = asyncio.run(hwinfo_product_proxy.get_info())
            return product_info_resp.info
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    # List all private methods
    def _send_log_command(
        self, tag: str, message: str, level: custom_types.LEVEL
    ) -> None:
        """Send a device command to write to the syslog.

        Args:
            tag: Tag to apply to the message in the syslog.
            message: Message that need to logged.
            level: Log message level.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            rcs_proxy = fd_remotecontrol.RemoteControl.Client(
                self.fuchsia_controller.ctx.connect_remote_control_proxy()
            )
            asyncio.run(
                rcs_proxy.log_message(
                    tag=tag, message=message, severity=_LOG_SEVERITIES[level]
                )
            )
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    def _send_reboot_command(self) -> None:
        """Send a device command to trigger a soft reboot.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure.
        """
        try:
            power_proxy = fhp_statecontrol.Admin.Client(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["PowerAdmin"]
                )
            )
            asyncio.run(
                power_proxy.reboot(
                    reason=fhp_statecontrol.RebootReason.USER_REQUEST
                )
            )
        except fcp.ZxStatus as status:
            # ZX_ERR_PEER_CLOSED is expected in this instance because the device
            # powered off.
            zx_status: int | None = (
                status.args[0] if len(status.args) > 0 else None
            )
            if zx_status != fcp.ZxStatus.ZX_ERR_PEER_CLOSED:
                raise errors.FuchsiaControllerError(
                    "Fuchsia Controller FIDL Error"
                ) from status

    def _read_snapshot_from_channel(self, channel_client: fcp.Channel) -> bytes:
        """Read snapshot data from client end of the transfer channel.

        Args:
            channel_client: Client end of the snapshot data channel.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure or on
              data transfer verification failure.

        Returns:
            Bytes containing snapshot data as a zip archive.
        """
        # Snapshot is sent over the channel as |fuchsia.io.File|.
        file_proxy = f_io.File.Client(channel_client)

        # Get file size for verification later.
        try:
            attr_resp: f_io.Node1GetAttrResponse = asyncio.run(
                file_proxy.get_attr()
            )
            if attr_resp.s != fcp.ZxStatus.ZX_OK:
                raise errors.FuchsiaControllerError(
                    f"get_attr() returned status: {attr_resp.s}"
                )
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError("get_attr() failed") from status

        # Read until channel is empty.
        ret: bytearray = bytearray()
        try:
            while True:
                result: f_io.ReadableReadResult = asyncio.run(
                    file_proxy.read(count=f_io.MAX_BUF)
                )
                if result.err:
                    raise errors.FuchsiaControllerError(
                        "read() failed. Received zx.Status {result.err}"
                    )
                if not result.response.data:
                    break
                ret.extend(result.response.data)
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError("read() failed") from status

        # Verify transfer.
        expected_size: int = attr_resp.attributes.content_size
        if len(ret) != expected_size:
            raise errors.FuchsiaControllerError(
                f"Expected {expected_size} bytes, but read {len(ret)} bytes"
            )

        return bytes(ret)

    def _send_snapshot_command(self) -> bytes:
        """Send a device command to take a snapshot.

        Raises:
            errors.FuchsiaControllerError: On FIDL communication failure or on
              data transfer verification failure.

        Returns:
            Bytes containing snapshot data as a zip archive.
        """
        # Ensure device is healthy and ready to send FIDL requests before
        # sending snapshot command.
        self.fuchsia_controller.create_context()
        self.health_check()

        channel_server, channel_client = fcp.Channel.create()
        params = f_feedback.GetSnapshotParameters(
            # Set timeout to 2 minutes in nanoseconds.
            collection_timeout_per_data=2 * 60 * 10**9,
            response_channel=channel_server.take(),
        )

        try:
            feedback_proxy = f_feedback.DataProvider.Client(
                self.fuchsia_controller.connect_device_proxy(
                    _FC_PROXIES["Feedback"]
                )
            )
            # The data channel isn't populated until get_snapshot() returns so
            # there's no need to drain the channel in parallel.
            asyncio.run(feedback_proxy.get_snapshot(params=params))
        except fcp.ZxStatus as status:
            raise errors.FuchsiaControllerError(
                "get_snapshot() failed"
            ) from status
        return self._read_snapshot_from_channel(channel_client)
