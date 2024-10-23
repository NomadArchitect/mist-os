# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by Honeydew."""

import logging

from honeydew.typing.wlan import RequestStatus

_LOGGER: logging.Logger = logging.getLogger(__name__)


class HoneydewError(Exception):
    """Base exception for Honeydew module.

    More specific exceptions will be created by inheriting from this exception.
    """

    def __init__(self, msg: str | Exception) -> None:
        """Inits HoneydewError with 'msg' (an error message string).

        Args:
            msg: an error message string or an Exception instance.

        Note: Additionally, logs 'msg' to debug log level file.
        """
        super().__init__(msg)
        _LOGGER.debug(repr(self), exc_info=True)


class ConfigError(HoneydewError):
    """Exception for reporting invalid config passed to Honeydew."""


class HostCmdError(HoneydewError):
    """Exception for reporting host command failures."""


class TransportError(HoneydewError):
    """Exception for errors raised by Honeydew transports."""


class HttpRequestError(TransportError):
    """Exception for errors raised by HTTP requests running on host machine."""


class HttpTimeoutError(HttpRequestError, TimeoutError):
    """Exception for errors raised by HTTP requests timing out on host machine."""


class Sl4fError(TransportError):
    """Exception for errors raised by SL4F requests."""


class Sl4fTimeoutError(Sl4fError, TimeoutError):
    """Exception for errors raised by SL4F request timeouts."""


class SerialError(TransportError):
    """Exception for errors raised by host-target communication over serial."""


class FfxCommandError(TransportError):
    """Exception for errors raised by ffx commands running on host machine."""


class FuchsiaControllerError(TransportError):
    """Exception for errors raised by Fuchsia Controller requests."""


class FastbootCommandError(HoneydewError):
    """Exception for errors raised by Fastboot commands."""


class TransportConnectionError(HoneydewError):
    """Raised when transport's check_connection fails."""


class Sl4fConnectionError(TransportConnectionError):
    """Raised when FFX transport's check_connection fails."""


class FfxConnectionError(TransportConnectionError):
    """Raised when FFX transport's check_connection fails."""


class FuchsiaControllerConnectionError(TransportConnectionError):
    """Raised when Fuchsia-Controller transport's check_connection fails."""


class FastbootConnectionError(TransportConnectionError):
    """Raised when Fastboot transport's check_connection fails."""


class FfxConfigError(HoneydewError):
    """Raised by ffx.FfxConfig class."""


class HoneydewTimeoutError(HoneydewError):
    """Exception for timeout based raised by Honeydew."""


class FfxTimeoutError(HoneydewTimeoutError):
    """Exception for timeout based errors raised by ffx commands running on host machine."""


class HoneydewRtcError(HoneydewError):
    """Raised by rtc.Rtc class."""


class HoneydewDataResourceError(HoneydewError):
    """Raised when Honeydew fails to fetch its data resources."""


class HoneydewWlanError(HoneydewError):
    """Raised by WLAN affordances."""


class HoneydewWlanRequestRejectedError(HoneydewWlanError):
    """WLAN stack rejected a request.

    Read the `reason` member variable for details on why this request has been
    rejected by the WLAN stack.
    """

    def __init__(self, method: str, reason: RequestStatus) -> None:
        """Initialize a HoneydewWlanRequestRejectedError.

        Args:
            name: name of the request that failed
            reason: additional information about the failed request.
        """
        super().__init__(f"{method} rejected with RequestStatus {reason}")
        self.reason = reason


class HoneydewNetstackError(HoneydewError):
    """Raised by netstack affordances."""


class HoneydewLocationError(HoneydewError):
    """Raised by location affordance."""


class NetworkInterfaceNotFoundError(HoneydewError):
    """Raised when a matching network interface is not found."""


class FuchsiaStateError(HoneydewError):
    """Exception for state errors."""


class FuchsiaDeviceError(HoneydewError):
    """Base exception for errors raised by fuchsia device."""


class SessionError(HoneydewError):
    """Exception for errors raised by Session."""


class DeviceNotConnectedError(HoneydewError):
    """Exception to be raised when device is not connected to host."""


class NotSupportedError(HoneydewError):
    """Exception to be raised if an operation is not yet supported by
    underlying Fuchsia platform."""


class StarnixError(HoneydewError):
    """Exception to be raised if a starnix operation fails."""


class UserInputError(HoneydewError):
    """Exception to be raised by UserInput"""


class BluetoothError(HoneydewError):
    """Exception to be raised if Bluetooth operation fails."""


class BluetoothStateError(HoneydewError):
    """Exception to be raised for unexpected Bluetooth states."""


class InspectError(HoneydewError):
    """Exception to be raised for Inspect affordance related failures."""
