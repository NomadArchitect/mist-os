// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use cm_rust::CapabilityTypeName;
use fuchsia_url::AbsolutePackageUrl;
use scrutiny::prelude::{DataCollector, DataModel};
use scrutiny_config::ModelConfig;
use scrutiny_plugins::core::collection::{Component, Components, Package, Packages};
use scrutiny_plugins::core::controller::package_extract::PackageExtractController;
use scrutiny_plugins::unified_plugin::UnifiedCollector;
use scrutiny_plugins::verify::controller::capability_routing::{
    CapabilityRouteController, ResponseLevel,
};
use scrutiny_plugins::verify::controller::structured_config::{
    ExtractStructuredConfigController, ExtractStructuredConfigResponse,
};
use scrutiny_plugins::verify::CapabilityRouteResults;
use scrutiny_utils::url::from_pkg_url_parts;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

pub struct Scrutiny {
    model_config: ModelConfig,
}

impl Scrutiny {
    pub fn from_product_bundle(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { model_config: ModelConfig::from_product_bundle(path)? })
    }

    pub fn from_product_bundle_recovery(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { model_config: ModelConfig::from_product_bundle_recovery(path)? })
    }

    pub fn set_component_tree_config_path(&mut self, path: impl AsRef<Path>) {
        self.model_config.component_tree_config_path = Some(path.as_ref().to_path_buf());
    }

    pub fn collect(self) -> Result<ScrutinyArtifacts> {
        let model = Arc::new(DataModel::new(self.model_config)?);
        let collector = UnifiedCollector::default();
        collector.collect(model.clone())?;
        Ok(ScrutinyArtifacts { model })
    }
}

pub struct ScrutinyArtifacts {
    model: Arc<DataModel>,
}

impl ScrutinyArtifacts {
    pub fn get_components(&self) -> Result<Vec<Component>> {
        let mut components = self.model.get::<Components>()?.entries.clone();
        components.sort_by(|a, b| a.url.partial_cmp(&b.url).unwrap());
        Ok(components)
    }

    pub fn get_package(&self, url: AbsolutePackageUrl) -> Result<Option<Package>> {
        let packages = &self.model.get::<Packages>()?.entries;
        for package in packages.iter() {
            if package.matches_url(&url) {
                return Ok(Some(package.clone()));
            }
        }
        return Ok(None);
    }

    pub fn get_package_urls(&self) -> Result<Vec<AbsolutePackageUrl>> {
        let mut packages = self.model.get::<Packages>().unwrap().entries.clone();
        packages.sort();
        Ok(packages
            .into_iter()
            .map(|package| from_pkg_url_parts(package.name, package.variant, Some(package.merkle)))
            .collect::<Result<Vec<AbsolutePackageUrl>>>()?)
    }

    pub fn extract_package(
        &self,
        url: AbsolutePackageUrl,
        output: impl AsRef<Path>,
    ) -> Result<Value> {
        PackageExtractController::extract(self.model.clone(), url, output)
    }

    pub fn extract_structured_config(&self) -> Result<ExtractStructuredConfigResponse> {
        ExtractStructuredConfigController::extract(self.model.clone())
    }

    pub fn get_capability_route_results(
        &self,
        capability_types: HashSet<CapabilityTypeName>,
        response_level: &ResponseLevel,
    ) -> Result<CapabilityRouteResults> {
        CapabilityRouteController::get_results(self.model.clone(), capability_types, response_level)
    }
}
