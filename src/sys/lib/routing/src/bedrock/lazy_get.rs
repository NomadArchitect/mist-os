// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{DictExt, RoutingError};
use async_trait::async_trait;
use cm_types::IterablePath;
use router_error::RouterError;
use sandbox::{Capability, Request, Routable, Router};
use std::fmt::Debug;

/// Implements the `lazy_get` function for [`Routable`] objects.
pub trait LazyGet: Routable {
    /// Returns a router that requests capabilities from the specified `path` relative to
    /// the base routable or fails the request with `not_found_error` if the member is not
    /// found. The base routable should resolve with a dictionary capability.
    fn lazy_get<P>(self, path: P, not_found_error: RoutingError) -> Router
    where
        P: IterablePath + Debug + 'static;
}

impl<T: Routable + 'static> LazyGet for T {
    fn lazy_get<P>(self, path: P, not_found_error: RoutingError) -> Router
    where
        P: IterablePath + Debug + 'static,
    {
        #[derive(Debug)]
        struct ScopedDictRouter<P: IterablePath + Debug + 'static> {
            router: Router,
            path: P,
            not_found_error: RoutingError,
        }

        #[async_trait]
        impl<P: IterablePath + Debug + 'static> Routable for ScopedDictRouter<P> {
            async fn route(
                &self,
                request: Option<Request>,
                debug: bool,
            ) -> Result<Capability, RouterError> {
                // If `debug` is true, that should only apply to the capability at `path`.
                // Here we're looking up the containing dictionary, so set `debug = false`, to
                // obtain the actual Dict and not its debug info.
                let init_request = request.as_ref().map(|r| r.try_clone()).transpose()?;
                match self.router.route(init_request, false).await? {
                    Capability::Dictionary(dict) => {
                        let request = request.as_ref().map(|r| r.try_clone()).transpose()?;
                        let maybe_capability = dict
                            .get_with_request(
                                self.not_found_error.clone(),
                                &self.path,
                                request,
                                debug,
                            )
                            .await?;
                        maybe_capability.ok_or_else(|| self.not_found_error.clone().into())
                    }
                    _ => Err(RoutingError::BedrockMemberAccessUnsupported {
                        moniker: self.not_found_error.clone().into(),
                    }
                    .into()),
                }
            }
        }

        Router::new(ScopedDictRouter {
            router: Router::new(self),
            path,
            not_found_error: not_found_error.into(),
        })
    }
}
