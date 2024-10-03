// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{ensure, Context};
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_config::power_config::PowerConfig;
use assembly_util::{BootfsDestination, FileEntry};

pub(crate) struct PowerManagementSubsystem;

impl DefineSubsystemConfiguration<PowerConfig> for PowerManagementSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &PowerConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if let Some(energy_model_config) = &context.board_info.configuration.energy_model {
            builder
                .bootfs()
                .file(FileEntry {
                    source: energy_model_config.as_utf8_pathbuf().into(),
                    destination: BootfsDestination::EnergyModelConfig,
                })
                .context("Adding energy model config file for processor power management")?;
        }

        if let Some(power_manager_config) = &context.board_info.configuration.power_manager {
            builder
                .bootfs()
                .file(FileEntry {
                    source: power_manager_config.as_utf8_pathbuf().into(),
                    destination: BootfsDestination::PowerManagerNodeConfig,
                })
                .context("Adding power_manager config file")?;
        }

        if let Some(system_power_mode_config) = &context.board_info.configuration.system_power_mode
        {
            builder
                .bootfs()
                .file(FileEntry {
                    source: system_power_mode_config.as_utf8_pathbuf().into(),
                    destination: BootfsDestination::SystemPowerModeConfig,
                })
                .context("Adding system power mode configuration file")?;
        }

        if let Some(thermal_config) = &context.board_info.configuration.thermal {
            builder
                .bootfs()
                .file(FileEntry {
                    source: thermal_config.as_utf8_pathbuf().into(),
                    destination: BootfsDestination::PowerManagerThermalConfig,
                })
                .context("Adding power_manager's thermal config file")?;
        }

        if *context.feature_set_level != FeatureSupportLevel::Embeddable {
            builder.platform_bundle("legacy_power_framework");
            if config.suspend_enabled {
                ensure!(*context.build_type != BuildType::User);
                builder.platform_bundle("power_framework");

                match context.feature_set_level {
                    FeatureSupportLevel::Embeddable | FeatureSupportLevel::Bootstrap => {}
                    FeatureSupportLevel::Utility | FeatureSupportLevel::Standard => {
                        // Include only when the base package set is available
                        builder.platform_bundle("power_framework_development_support");
                    }
                }

                match config.testing_sag_enabled {
                    true => {
                        builder.platform_bundle("power_framework_testing_sag");
                    }
                    false => {
                        builder.platform_bundle("power_framework_sag");
                        builder.platform_bundle("topology_test_daemon");
                    }
                }
            }
            if let Some(cpu_manager_config) = &context.board_info.configuration.cpu_manager {
                builder.platform_bundle("cpu_manager");
                builder
                    .bootfs()
                    .file(FileEntry {
                        source: cpu_manager_config.as_utf8_pathbuf().into(),
                        destination: BootfsDestination::CpuManagerNodeConfig,
                    })
                    .context("Adding cpu_manager config file")?;
            }
        }

        builder.set_config_capability(
            "fuchsia.power.SuspendEnabled",
            Config::new(ConfigValueType::Bool, config.suspend_enabled.into()),
        )?;

        if let (Some(config), FeatureSupportLevel::Standard) =
            (&context.board_info.configuration.power_metrics_recorder, &context.feature_set_level)
        {
            builder.platform_bundle("power_metrics_recorder");
            builder.package("metrics-logger-standalone").config_data(FileEntry {
                source: config.as_utf8_pathbuf().into(),
                destination: "config.json".to_string(),
            })?;
        }

        // Include fake-battery driver through a platform AIB.
        if context.board_info.provides_feature("fuchsia::fake_battery") {
            // We only need this driver feature in the utility / standard feature set levels.
            if *context.feature_set_level == FeatureSupportLevel::Standard
                || *context.feature_set_level == FeatureSupportLevel::Utility
            {
                builder.platform_bundle("fake_battery_driver");
            }
        }

        Ok(())
    }
}
