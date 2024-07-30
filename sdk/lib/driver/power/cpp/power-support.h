// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_POWER_SUPPORT_H_
#define LIB_DRIVER_POWER_CPP_POWER_SUPPORT_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fit/function.h>
#include <lib/zx/event.h>
#include <lib/zx/handle.h>

/// Collection of helpers for driver authors working with the power framework.
/// The basic usage model is
///   * use `fuchsia.hardware.platform.device/Device.GetPowerConfiguration` to
///     retrieve the config supplied by the board driver.
///   * For each power element in the driver's config
///       - Call `PowerAdapter::GetDependencyTokens` to get the element's
///         parents' access tokens.
///       - Calling `PowerAdapter::AddElement` and supplying the configuration,
///         token set from `GetDependencyTokens` and any access tokens the
///         driver needs to declare.
namespace fdf_power {

enum class Error : uint8_t {
  /// The power configuration appears to be invalid. A non-exhaustive list of
  /// possible reasons is it contained no elements, the element definition
  /// appears malformed, or other reasons.
  INVALID_ARGS,
  /// A general I/O error happened which we're not sure about. This should be
  /// a rare occurrence and typically more specific errors should be returned.
  IO,
  /// The configuration has a dependency, but we couldn't get access to the
  /// tokens for it. Maybe a parent didn't offer something expected or SAG
  /// didn't make something available.
  DEPENDENCY_NOT_FOUND,
  /// No token services capability available, maybe it wasn't routed?
  TOKEN_SERVICE_CAPABILITY_NOT_FOUND,
  /// An unexpected error occurred listing service instances.
  READ_INSTANCES,
  /// We were able to access the token service capability, but no instances
  /// were available. Did the parents offer any?
  NO_TOKEN_SERVICE_INSTANCES,
  /// Requesting a token from the provider protocol failed. Maybe the token
  /// provider is not implemented correctly?
  TOKEN_REQUEST,
  /// Couldn't access the capability for System Activity Governor tokens.
  ACTIVITY_GOVERNOR_UNAVAILABLE,
  /// Request to System Activity Governor returned an error.
  ACTIVITY_GOVERNOR_REQUEST,
};

enum class ElementRunnerError : uint8_t {
  /// Maps to fuchsia.power.broker/RequiredLevelError::INTERNAL
  REQUIRED_LEVEL_INTERNAL,
  /// Maps to fuchsia.power.broker/RequiredLevelError::NOT_AUTHORIZED
  REQUIRED_LEVEL_NOT_AUTHORIZED,
  /// Maps to fuchsia.power.broker/RequiredLevelError::UNKNOWN
  REQUIRED_LEVEL_UNKNOWN,
  /// fuchsia.power.broker/RequiredLevelError has a value we don't recognize
  REQUIRED_LEVEL_UNEXPECTED,
  /// The fuchsia.power.broker/RequiredLevel channel closed
  REQUIRED_LEVEL_TRANSPORT_PEER_CLOSED,
  /// The fuchsia.power.broker/RequiredLevel had a FIDL transport error other
  /// than closed.
  REQUIRED_LEVEL_TRANSPORT_OTHER,
  /// Maps to fuchsia.power.broker/CurrentLevelError::NOT_AUTHORIZED
  CURRENT_LEVEL_NOT_AUTHORIZED,
  /// fuchsia.power.broker/CurrentLevelError has a value we don't recognize
  CURRENT_LEVEL_UNEXPECTED,
  /// The fuchsia.power.broker/CurrentLevel channel closed
  CURRENT_LEVEL_TRANSPORT_PEER_CLOSED,
  /// The fuchsia.power.broker/CurrentLevel had a FIDL transport error other
  /// than closed.
  CURRENT_LEVEL_TRANSPORT_OTHER,
  /// The level change callback returned an error
  LEVEL_CHANGE_CALLBACK,
};

class ParentElementHasher final {
 public:
  /// Make a unique string as our hash key. The string is the ordinal of the SAG
  /// value or 0 if not present followed by a forward slash, followed by the
  /// parent name, or the empty string if name is not present.
  /// [${sag}|0]/[${name}]
  size_t operator()(const fuchsia_hardware_power::ParentElement& element) const;
};

using TokenMap =
    std::unordered_map<fuchsia_hardware_power::ParentElement, zx::event, ParentElementHasher>;

using ElementDependencyMap =
    std::unordered_map<fuchsia_hardware_power::ParentElement,
                       std::vector<fuchsia_power_broker::LevelDependency>, ParentElementHasher>;

struct ElementDesc {
  fuchsia_hardware_power::wire::PowerElementConfiguration element_config_;
  TokenMap tokens_;
  zx::event assertive_token_;
  zx::event opportunistic_token_;
  std::pair<fidl::ServerEnd<fuchsia_power_broker::CurrentLevel>,
            fidl::ServerEnd<fuchsia_power_broker::RequiredLevel>>
      level_control_servers_;
  fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor_server_;
  fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control_server_;

  // The below are created if the caller did not supply their corresponding server end
  std::optional<fidl::ClientEnd<fuchsia_power_broker::CurrentLevel>> current_level_client_;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::RequiredLevel>> required_level_client_;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::Lessor>> lessor_client_;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::ElementControl>> element_control_client_;
};

/// Runs a power element.
///
/// Once |RunPowerElement| is called, this object listens for new levels
/// reported to it via |RequiredLevel.Watch|, calls the provided
/// |level_change_callback|, and reports the level returned by that callback
/// via |CurrentLevel.Update|. This object stops running the power element if
/// an error occurs and reports the error via |error_handler|. This class is
/// not thread-safe and should created on a thread with a synchronized
/// dispatcher and given a pointer to that same dispatcher. Since the callbacks
/// happen on the same dispatcher, blocking the callbacks blocks runnng the
/// element. Calls to |SetLevel| do not trigger a |level_change_callback|
/// invocation.
class ElementRunner {
 public:
  ElementRunner(fidl::ClientEnd<fuchsia_power_broker::RequiredLevel> required_level,
                fidl::ClientEnd<fuchsia_power_broker::CurrentLevel> current_level,
                fit::function<fit::result<zx_status_t, uint8_t>(uint8_t)> level_change_callback,
                fit::function<void(ElementRunnerError)> error_handler,
                async_dispatcher_t* dispatcher) {
    on_error_ = std::move(error_handler);
    on_level_change_ = std::move(level_change_callback);
    required_level_client_ =
        fidl::Client<fuchsia_power_broker::RequiredLevel>(std::move(required_level), dispatcher);
    current_level_client_ =
        fidl::Client<fuchsia_power_broker::CurrentLevel>(std::move(current_level), dispatcher);
  }

  /// Runs the power element asynchronously. The object continues running the
  /// power element until an error occurs or the object is destroyed. Running
  /// the element *and* making callbacks via |level_change_callback| and
  /// |error_handler| are done on the object's dispatcher.
  ///
  /// The object listens for new levels, calls |level_change_callback| when one
  /// is received, reports the power level returned from |level_change_callback|
  /// via the |current_level| channel provided to the constructor, and calls
  /// |error_handler| if an error occurs.
  ///
  /// After |error_handler| is called, this object stops running the element.
  /// |RunPowerElement| can then be called again to continue running it.
  void RunPowerElement();

  /// Sets the level of the element via the |CurrentLevel| channel provided to
  /// the constructor. The call returns immediately and the result is delivered
  /// to |callback| on the |dispatcher| passed to the constructor.
  void SetLevel(
      uint8_t level,
      fit::function<void(
          fit::result<fidl::ErrorsIn<fuchsia_power_broker::CurrentLevel::Update>, zx_status_t>)>
          callback);

 private:
  fidl::Client<fuchsia_power_broker::RequiredLevel> required_level_client_;
  fidl::Client<fuchsia_power_broker::CurrentLevel> current_level_client_;
  fit::function<fit::result<zx_status_t, uint8_t>(uint8_t)> on_level_change_;
  fit::function<void(ElementRunnerError)> on_error_;
};

inline fit::result<zx_status_t, uint8_t> default_level_changer(uint8_t level) {
  return fit::ok(level);
}

/// Given a `PowerElementConfiguration` from driver framework, convert this
/// into a set of Power Broker's `LevelDependency` objects. The map is keyed
/// by the name of the parent/dependency.
///
/// If the `PowerElementConfiguration` expresses no dependencies, we return an
/// empty map.
///
/// NOTE: The `requires_token` of each of the `LevelDependency` objects is
/// **not** populated and must be filled in before providing this map to
/// `AddElement`.
///
/// Error returns:
///   - Error::INVALID_ARGS if `element_config` is missing fields, for example
///     if a level dependency doesn't have a parent level.
fit::result<Error, ElementDependencyMap> LevelDependencyFromConfig(
    fuchsia_hardware_power::wire::PowerElementConfiguration element_config);

/// Given a `PowerElementConfiguration` from driver framework, convert this
/// into a set of Power Broker's `PowerLevel` objects.
///
/// If the `PowerElementConfiguration` expresses no levels, we return an
/// empty vector.
std::vector<fuchsia_power_broker::PowerLevel> PowerLevelsFromConfig(
    fuchsia_hardware_power::wire::PowerElementConfiguration element_config);

/// For the Power Element represented by `element_config`, get the tokens for
/// the element's dependencies (ie. "parents") from
/// `fuchsia.hardware.power/PowerTokenProvider` instances in `ns`.
///
/// If the power element represented by `element_config` has no dependencies,
/// this function returns an empty set. If any dependency's token can not be
/// be retrieved we return an error.
/// Error returns:
///   - `Error::INVALID_ARGS` if the element_config appears invalid
///   - `Error::IO` if there is a communication failure when talking to a
///      service or a protocol required to get a token.
///   - `Error::DEPENDENCY_NOT_FOUND` if a token for a required dependency is
///     not available.
fit::result<Error, TokenMap> GetDependencyTokens(
    const fdf::Namespace& ns,
    fuchsia_hardware_power::wire::PowerElementConfiguration element_config);

/// For the Power Element represented by `element_config`, get the tokens for
/// the
/// element's dependencies (ie. "parents") from
/// `fuchsia.hardware.power/PowerTokenProvider` instances in `svcs_dir`.
/// `svcs_dir` should contain an entry for
/// `fuchsia.hardware.power/PowerTokenService`.
///
/// Returns a set of tokens from services instances found in `svcs_dir`. If
/// the power element represented by `element_config` has no dependencies, this
/// function returns an empty set. If any dependency's token can not be
/// be retrieved we return an error.
/// Error returns:
///   - `Error::INVALID_ARGS` if the element_config appears invalid
///   - `Error::IO` if there is a communication failure when talking to a
///      service or a protocol required to get a token.
///   - `Error::DEPENDENCY_NOT_FOUND` if a token for a required dependency is
///     not available.
fit::result<Error, TokenMap> GetDependencyTokens(
    fuchsia_hardware_power::wire::PowerElementConfiguration element_config,
    fidl::ClientEnd<fuchsia_io::Directory> svcs_dir);

/// Call `AddElement` on the `power_broker` channel passed in.
/// This function uses the `config` and `tokens` arguments to properly construct
/// the call to `fuchsia.power.broker/Topology.AddElement`. Optionally callers
/// can pass in tokens to be registered for granting assertive and opportunistic
/// dependency access on the created element.
///
/// Error
///   - Error::DEPENDENCY_NOT_FOUND if there is a dependency specified by
///     `config` which is to found in `tokens`.
///   - Error::INVALID_ARGS if `config` appears to be invalid, we fail to
///     duplicate a token and therefore assume it must have been invalid, or
///     the call to power broker fails for any reason *other* than a closed
///     channel.
fit::result<Error> AddElement(
    const fidl::ClientEnd<fuchsia_power_broker::Topology>& power_broker,
    fuchsia_hardware_power::wire::PowerElementConfiguration config, TokenMap tokens,
    const zx::unowned_event& assertive_token, const zx::unowned_event& opportunistic_token,
    std::optional<std::pair<fidl::ServerEnd<fuchsia_power_broker::CurrentLevel>,
                            fidl::ServerEnd<fuchsia_power_broker::RequiredLevel>>>
        level_control,
    std::optional<fidl::ServerEnd<fuchsia_power_broker::Lessor>> lessor,
    std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementControl>> element_control);

/// Call `AddElement` on the `power_broker` channel passed in.
/// This function uses `ElementDescription` passed in to make the proper call
/// to `fuchsia.power.broker/Topology.AddElement`. See `ElementDescription` for
/// more information about what fields are inputs to `AddElement`.
///
/// Error
///   - Error::DEPENDENCY_NOT_FOUND if there is a dependency specified by
///     `config` which is to found in `tokens`.
///   - Error::INVALID_ARGS if `config` appears to be invalid, we fail to
///     duplicate a token and therefore assume it must have been invalid, or
///     the call to power broker fails for any reason *other* than a closed
///     channel.
fit::result<Error> AddElement(fidl::ClientEnd<fuchsia_power_broker::Topology>& power_broker,
                              ElementDesc& description);
}  // namespace fdf_power

#endif  // LIB_DRIVER_POWER_CPP_POWER_SUPPORT_H_
