// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDIO_DIRECTORY_H_
#define LIB_FDIO_DIRECTORY_H_

#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <stdint.h>
#include <unistd.h>
#include <zircon/analyzer.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Connects to a service at `path` relative to the root of the namespace for the current process
// asynchronously.
//
// `request` must be a channel.
//
// Always consumes `request`.
//
// See `fdio_ns_service_connect` for details.
zx_status_t fdio_service_connect(const char* path, ZX_HANDLE_RELEASE zx_handle_t request)
    ZX_AVAILABLE_SINCE(1);

// Connects to a service at the given `path` relative to the given `directory` asynchronously.
//
// Upon success, the `request` is handed off to the remote party. The operation completes
// asynchronously, which means a ZX_OK result does not ensure that the requested service actually
// exists.
//
// `directory` must be a channel that implements the `fuchsia.io/Directory` protocol.
//
// `request` must be a channel. It will always be consumed by this function.
//
// # Errors
//
// ZX_ERR_INVALID_ARGS: `directory` or `path` is invalid.
zx_status_t fdio_service_connect_at(zx_handle_t directory, const char* path,
                                    ZX_HANDLE_RELEASE zx_handle_t request) ZX_AVAILABLE_SINCE(1);

// Connect to a service named `name` in /svc.
zx_status_t fdio_service_connect_by_name(const char* name, ZX_HANDLE_RELEASE zx_handle_t request)
    ZX_AVAILABLE_SINCE(1);

// Opens an object at `path` relative to the root of the namespace for the current process with
// `flags` asynchronously.
//
// `flags` is a `fuchsia.io/OpenFlags`.
//
// Always consumes `request`.
//
// See `fdio_ns_open` for details.
zx_status_t fdio_open(const char* path, uint32_t flags, ZX_HANDLE_RELEASE zx_handle_t request)
    ZX_AVAILABLE_SINCE(1);

// Opens an object at `path` relative to `directory` with `flags` asynchronously.
//
// Upon success, `request` is handed off to the remote party. The operation completes
// asynchronously, which means a ZX_OK result does not ensure that the requested service actually
// exists.
//
// `directory` must be a channel that implements the `fuchsia.io/Directory` protocol.
//
// `request` must be a channel which will always be consumed by this function.
//
// # Errors
//
// ZX_ERR_INVALID_ARGS: `directory` or `path` is invalid.
zx_status_t fdio_open_at(zx_handle_t directory, const char* path, uint32_t flags,
                         ZX_HANDLE_RELEASE zx_handle_t request) ZX_AVAILABLE_SINCE(1);

// Opens an object at `path` relative to the root of the namespace for the current process with
// `flags` synchronously, and on success, binds that channel to a file descriptor, returned via
// `out_fd`.
//
// Note that unlike `fdio_open`, this function is synchronous. This is because it produces a file
// descriptor, which requires synchronously waiting for the open to complete.
//
// `flags` is a `fuchsia.io/OpenFlags`.
//
// See `fdio_open` for details.
zx_status_t fdio_open_fd(const char* path, uint32_t flags, int* out_fd) ZX_AVAILABLE_SINCE(1);

// Opens an object at `path` relative to `dir_fd` with `flags` synchronously, and on success, binds
// that channel to a file descriptor, returned via `out_fd`.
//
// Note that unlike fdio_open, this function is synchronous. This is because it produces a file
// descriptor, which requires synchronously waiting for the open to complete.
//
// `flags` is a `fuchsia.io/OpenFlags`.
//
// See `fdio_open_at` fort details.
zx_status_t fdio_open_fd_at(int dir_fd, const char* path, uint32_t flags, int* out_fd)
    ZX_AVAILABLE_SINCE(1);

__END_CDECLS

#endif  // LIB_FDIO_DIRECTORY_H_
