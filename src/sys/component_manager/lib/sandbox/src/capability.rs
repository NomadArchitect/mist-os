// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use from_enum::FromEnum;
use router_error::Explain;
use std::fmt::Debug;
use thiserror::Error;
use zx_status;

#[derive(Error, Debug, Clone)]
pub enum ConversionError {
    #[error("invalid `fuchsia.io` node name: name `{0}` is too long")]
    ParseNameErrorTooLong(String),

    #[error("invalid `fuchsia.io` node name: name cannot be empty")]
    ParseNameErrorEmpty,

    #[error("invalid `fuchsia.io` node name: name cannot be `.`")]
    ParseNameErrorDot,

    #[error("invalid `fuchsia.io` node name: name cannot be `..`")]
    ParseNameErrorDotDot,

    #[error("invalid `fuchsia.io` node name: name cannot contain `/`")]
    ParseNameErrorSlash,

    #[error("invalid `fuchsia.io` node name: name cannot contain embedded NUL")]
    ParseNameErrorEmbeddedNul,

    #[error("conversion to type is not supported")]
    NotSupported,

    #[error("value at `{key}` could not be converted")]
    Nested {
        key: String,
        #[source]
        err: Box<ConversionError>,
    },
}

#[cfg(target_os = "fuchsia")]
impl From<vfs::name::ParseNameError> for ConversionError {
    fn from(parse_name_error: vfs::name::ParseNameError) -> Self {
        match parse_name_error {
            vfs::name::ParseNameError::TooLong(s) => ConversionError::ParseNameErrorTooLong(s),
            vfs::name::ParseNameError::Empty => ConversionError::ParseNameErrorEmpty,
            vfs::name::ParseNameError::Dot => ConversionError::ParseNameErrorDot,
            vfs::name::ParseNameError::DotDot => ConversionError::ParseNameErrorDotDot,
            vfs::name::ParseNameError::Slash => ConversionError::ParseNameErrorSlash,
            vfs::name::ParseNameError::EmbeddedNul => ConversionError::ParseNameErrorEmbeddedNul,
        }
    }
}

/// Errors arising from conversion between Rust and FIDL types.
#[derive(Error, Debug)]
pub enum RemoteError {
    #[error("unknown FIDL variant")]
    UnknownVariant,

    #[error("unregistered capability; only capabilities created by sandbox are allowed")]
    Unregistered,

    #[error("registered capability had the wrong type")]
    BadCapability,
}

impl Explain for RemoteError {
    fn as_zx_status(&self) -> zx_status::Status {
        match self {
            RemoteError::UnknownVariant => zx_status::Status::NOT_SUPPORTED,
            RemoteError::Unregistered => zx_status::Status::INVALID_ARGS,
            RemoteError::BadCapability => zx_status::Status::INVALID_ARGS,
        }
    }
}

#[derive(FromEnum, Debug)]
pub enum Capability {
    Unit(crate::Unit),
    Connector(crate::Connector),
    Dictionary(crate::Dict),
    Data(crate::Data),
    Directory(crate::Directory),
    Handle(crate::Handle),
    Router(crate::Router),
    Instance(crate::WeakInstanceToken),

    #[cfg(target_os = "fuchsia")]
    DirEntry(crate::DirEntry),
}

impl Capability {
    pub fn to_dictionary(self) -> Option<crate::Dict> {
        match self {
            Self::Dictionary(d) => Some(d),
            _ => None,
        }
    }

    pub fn try_clone(&self) -> Result<Self, ()> {
        let out = match self {
            Self::Connector(s) => Self::Connector(s.clone()),
            Self::Router(s) => Self::Router(s.clone()),
            Self::Dictionary(s) => Self::Dictionary(s.clone()),
            Self::Data(s) => Self::Data(s.clone()),
            Self::Unit(s) => Self::Unit(s.clone()),
            Self::Directory(s) => Self::Directory(s.clone()),
            Self::Handle(s) => Self::Handle(s.try_clone()?),
            Self::Instance(s) => Self::Instance(s.clone()),
            #[cfg(target_os = "fuchsia")]
            Self::DirEntry(s) => Self::DirEntry(s.clone()),
        };
        Ok(out)
    }

    pub fn debug_typename(&self) -> &'static str {
        match self {
            Self::Connector(_) => "Sender",
            Self::Router(_) => "Router",
            Self::Dictionary(_) => "Dictionary",
            Self::Data(_) => "Data",
            Self::Unit(_) => "Unit",
            Self::Directory(_) => "Directory",
            Self::Handle(_) => "Handle",
            Self::Instance(_) => "Instance",

            #[cfg(target_os = "fuchsia")]
            Self::DirEntry(_) => "DirEntry",
        }
    }
}
