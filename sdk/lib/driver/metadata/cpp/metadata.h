// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_METADATA_CPP_METADATA_H_
#define LIB_DRIVER_METADATA_CPP_METADATA_H_

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_metadata {

// This template class is explicitly specialized and defines `Name` so that the service that offers
// |FidlType| can be routed by `MetadataServer` and found by `fdf_metadata::GetMetadata()`. The
// value can be anything so long as it is the same as the name of the service offered/used by the
// components that send/receive |FidlType|. Typically it will be related to the name of |FidlType|.
//
// For example, say there exists a FIDL type `fuchsia.hardware.test/Metadata` that is to be sent
// with `fdf_metadata::MetadataServer<|FidlType|>` and received with
// `fdf_metadata::GetMetadata<|FidlType|>()`:
//
//   library fuchsia.hardware.test;
//
//   type Metadata = table {
//       1: test_property string:MAX;
//   };
//
// There should be an `fdf_metadata::ObjectDetails<fuchsia_hardware_test::Metadata>`
// class that defines `Name` like so:
//
//   namespace fdf_metadata {
//
//     template <>
//     struct ObjectDetails<fuchsia_hardware_test::Metadata> {
//       inline static const char* Name = "fuchsia.hardware.test.Metadata";
//     };
//
//   }  // namespace fdf_metadata
//
// This struct can be defined in a header file that can be included by the components that use
// `fdf_metadata::MetadataServer<|FidlType|>` and `fdf_metadata::GetMetadata<|FidlType|>()`.
template <typename FidlType>
struct ObjectDetails {
  inline static const char* Name;
};

// Connects to the fuchsia.driver.metadata/Metadata FIDL protocol found within the |incoming|
// incoming namespace at FIDL service `ddk::ObjectDetails<|FidlType|>::Name` and instance
// |instance_name|.
template <typename FidlType>
zx::result<fidl::ClientEnd<fuchsia_driver_metadata::Metadata>> ConnectToMetadataProtocol(
    const std::shared_ptr<fdf::Namespace>& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  static_assert(fidl::IsFidlType<FidlType>::value, "|FidlType| must be a FIDL domain object.");

  static const char* kFidlServiceName = ObjectDetails<FidlType>::Name;

  // The metadata protocol is found within the `kFidlServiceName` service directory and not the
  // `fuchsia_driver_metadata::Service::Name` directory because that is where
  // `fdf_metadata::MetadataServer` is expected to serve the fuchsia.driver.metadata/Metadata'
  // protocol.
  auto path = std::string{kFidlServiceName}
                  .append("/")
                  .append(instance_name)
                  .append("/")
                  .append(fuchsia_driver_metadata::Service::Metadata::Name);

  zx::result result =
      component::ConnectAt<fuchsia_driver_metadata::Metadata>(incoming->svc_dir(), path);
  if (result.is_error()) {
    FDF_SLOG(ERROR, "Failed to connect to metadata protocol.", KV("status", result.status_string()),
             KV("path", path));
    return result.take_error();
  }

  return zx::ok(std::move(result.value()));
}

// Retrieves metadata from the fuchsia.driver.metadata/Metadata FIDL protocol within the |incoming|
// incoming namespace found at FIDL service `fdf_metadata::ObjectDetails<|FidlType|>::Name` and
// instance |instance_name|.
//
// Make sure that the component manifest specifies that it uses the
// `fdf_metadata::ObjectDetails<|FidlType|>::Name` FIDL service.
template <typename FidlType>
zx::result<FidlType> GetMetadata(
    const std::shared_ptr<fdf::Namespace>& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  static_assert(fidl::IsFidlType<FidlType>::value, "|FidlType| must be a FIDL domain object.");
  static_assert(!fidl::IsResource<FidlType>::value,
                "|FidlType| cannot be a resource type. Resources cannot be persisted.");

  fidl::WireSyncClient<fuchsia_driver_metadata::Metadata> client{};
  {
    zx::result result = ConnectToMetadataProtocol<FidlType>(incoming, instance_name);
    if (result.is_error()) {
      FDF_SLOG(ERROR, "Failed to connect to metadata server.",
               KV("status", result.status_string()));
      return result.take_error();
    }
    client.Bind(std::move(result.value()));
  }

  fidl::WireResult metadata_bytes = client->GetMetadata();
  if (!metadata_bytes.ok()) {
    FDF_SLOG(ERROR, "Failed to send GetMetadata request.",
             KV("status", metadata_bytes.status_string()));
    return zx::error(metadata_bytes.status());
  }
  if (metadata_bytes->is_error()) {
    FDF_SLOG(ERROR, "Failed to get metadata bytes.",
             KV("status", zx_status_get_string(metadata_bytes->error_value())));
    return zx::error(metadata_bytes->error_value());
  }

  fit::result metadata = fidl::Unpersist<FidlType>(metadata_bytes.value()->metadata.get());
  if (metadata.is_error()) {
    FDF_SLOG(ERROR, "Failed to unpersist metadata.",
             KV("status", zx_status_get_string(metadata.error_value().status())));
    return zx::error(metadata.error_value().status());
  }

  return zx::ok(metadata.value());
}

// This function is the same as `fdf_metadata::GetMetadata<FidlType>()` except that it will return a
// `std::nullopt` if there is no metadata FIDL protocol within |device|'s incoming namespace at
// |instance_name|.
template <typename FidlType>
zx::result<std::optional<FidlType>> GetMetadataIfExists(
    const std::shared_ptr<fdf::Namespace>& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  static_assert(fidl::IsFidlType<FidlType>::value, "|FidlType| must be a FIDL domain object.");
  static_assert(!fidl::IsResource<FidlType>::value,
                "|FidlType| cannot be a resource type. Resources cannot be persisted.");

  fidl::WireSyncClient<fuchsia_driver_metadata::Metadata> client{};
  {
    zx::result result = ConnectToMetadataProtocol<FidlType>(incoming, instance_name);
    if (result.is_error()) {
      FDF_SLOG(DEBUG, "Failed to connect to metadata server.",
               KV("status", result.status_string()));
      return zx::ok(std::nullopt);
    }
    client.Bind(std::move(result.value()));
  }

  fidl::WireResult metadata_bytes = client->GetMetadata();
  if (!metadata_bytes.ok()) {
    FDF_SLOG(DEBUG, "Failed to send GetMetadata request.",
             KV("status", metadata_bytes.status_string()));
    return zx::ok(std::nullopt);
  }
  if (metadata_bytes->is_error()) {
    FDF_SLOG(ERROR, "Failed to get metadata bytes.",
             KV("status", zx_status_get_string(metadata_bytes->error_value())));
    return zx::error(metadata_bytes->error_value());
  }

  fit::result metadata = fidl::Unpersist<FidlType>(metadata_bytes.value()->metadata.get());
  if (metadata.is_error()) {
    FDF_SLOG(ERROR, "Failed to unpersist metadata.",
             KV("status", zx_status_get_string(metadata.error_value().status())));
    return zx::error(metadata.error_value().status());
  }

  return zx::ok(metadata.value());
}

}  // namespace fdf_metadata

#endif

#endif  // LIB_DRIVER_METADATA_CPP_METADATA_H_
