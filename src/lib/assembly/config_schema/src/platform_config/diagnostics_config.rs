// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use camino::Utf8PathBuf;
use fuchsia_url::boot_url::BootUrl;
use fuchsia_url::AbsoluteComponentUrl;
use moniker::Moniker;
use schemars::JsonSchema;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

/// Diagnostics configuration options for the diagnostics area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsConfig {
    #[serde(default)]
    pub archivist: Option<ArchivistConfig>,
    /// The set of pipeline config files to supply to archivist.
    #[serde(default)]
    pub archivist_pipelines: Vec<ArchivistPipeline>,
    #[serde(default)]
    pub additional_serial_log_components: Vec<String>,
    #[serde(default)]
    pub sampler: SamplerConfig,
    #[serde(default)]
    pub memory_monitor: MemoryMonitorConfig,
    /// The set of log levels components will receive as their initial interest.
    #[serde(default)]
    pub component_log_initial_interests: Vec<ComponentInitialInterest>,
}

/// Diagnostics configuration options for the archivist configuration area.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub enum ArchivistConfig {
    Default,
    LowMem,
}

/// A single archivist pipeline config.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchivistPipeline {
    /// The name of the pipeline.
    pub name: PipelineType,
    /// The files to add to the pipeline.
    /// Zero files is not valid.
    #[schemars(schema_with = "crate::vec_path_schema")]
    pub files: Vec<Utf8PathBuf>,
}

#[derive(Debug, PartialEq, Clone, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(into = "String")]
pub enum PipelineType {
    /// A pipeline for feedback data.
    Feedback,
    /// A custom pipeline.
    Custom(String),
}

impl Serialize for PipelineType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Feedback => serializer.serialize_str("feedback"),
            Self::Custom(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for PipelineType {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?.to_lowercase();
        match variant.as_str() {
            "feedback" => Ok(PipelineType::Feedback),
            "lowpan" => Ok(PipelineType::Custom("lowpan".into())),
            "legacy_metrics" => Ok(PipelineType::Custom("legacy_metrics".into())),
            other => {
                Err(D::Error::unknown_variant(other, &["feedback", "lowpan", "legacy_metrics"]))
            }
        }
    }
}

impl std::fmt::Display for PipelineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Feedback => "feedback",
                Self::Custom(name) => &name,
            }
        )
    }
}

impl From<PipelineType> for String {
    fn from(t: PipelineType) -> Self {
        t.to_string()
    }
}

/// Diagnostics configuration options for the sampler configuration area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SamplerConfig {
    /// The metrics configs to pass to sampler.
    #[serde(default)]
    #[schemars(schema_with = "crate::vec_path_schema")]
    pub metrics_configs: Vec<Utf8PathBuf>,
    /// The fire configs to pass to sampler.
    #[serde(default)]
    #[schemars(schema_with = "crate::vec_path_schema")]
    pub fire_configs: Vec<Utf8PathBuf>,
}

/// Diagnostics configuration options for the memory monitor configuration area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryMonitorConfig {
    /// The memory buckets config file to provide to memory monitor.
    #[serde(default)]
    #[schemars(schema_with = "crate::option_path_schema")]
    pub buckets: Option<Utf8PathBuf>,
    /// Control whether a pressure change should trigger a capture.
    #[serde(default)]
    pub capture_on_pressure_change: Option<bool>,
    /// Expected delay between scheduled captures upon imminent OOM, in
    /// seconds.
    #[serde(default)]
    pub imminent_oom_capture_delay_s: Option<u32>,
    /// Expected delay between scheduled captures when the memory
    /// pressure is critical, in seconds.
    #[serde(default)]
    pub critical_capture_delay_s: Option<u32>,
    /// Expected delay between scheduled captures when the memory
    /// pressure is abnormal, in seconds.
    #[serde(default)]
    pub warning_capture_delay_s: Option<u32>,
    /// Expected delay between scheduled captures when the memory
    /// pressure is normal, in seconds.
    #[serde(default)]
    pub normal_capture_delay_s: Option<u32>,
}

/// The initial log interest that a component should receive upon starting up.
#[derive(Debug, Deserialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentInitialInterest {
    /// The URL or moniker for the component which should receive the initial interest.
    pub component: UrlOrMoniker,
    /// The log severity the initial interest should specify.
    pub log_severity: LogSeverity,
}

impl Serialize for ComponentInitialInterest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(format!("{}:{}", self.component, self.log_severity).as_str())
    }
}

#[derive(Debug, PartialEq, JsonSchema)]
pub enum UrlOrMoniker {
    /// An absolute fuchsia url to a component.
    Url(String),
    /// The absolute moniker for a component.
    Moniker(String),
}

impl std::fmt::Display for UrlOrMoniker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(u) => write!(f, "{}", u),
            Self::Moniker(m) => write!(f, "{}", m),
        }
    }
}

impl<'de> Deserialize<'de> for UrlOrMoniker {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?.to_lowercase();
        if AbsoluteComponentUrl::from_str(&variant).is_ok()
            || BootUrl::parse(variant.as_str()).is_ok()
        {
            Ok(UrlOrMoniker::Url(variant))
        } else if Moniker::from_str(&variant).is_ok() {
            Ok(UrlOrMoniker::Moniker(variant))
        } else {
            Err(D::Error::custom(format_args!(
                "Expected a moniker or url. {} was neither",
                variant
            )))
        }
    }
}

// LINT.IfChange
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogSeverity {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}
// LINT.ThenChange(/src/lib/diagnostics/data/rust/src/lib.rs)

impl std::fmt::Display for LogSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trace => write!(f, "trace"),
            Self::Debug => write!(f, "debug"),
            Self::Info => write!(f, "info"),
            Self::Warning => write!(f, "warn"),
            Self::Error => write!(f, "error"),
            Self::Fatal => write!(f, "fatal"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_config::PlatformConfig;

    use assembly_util as util;

    #[test]
    fn test_diagnostics_archivist_default_service() {
        let json5 = r#""default""#;
        let mut cursor = std::io::Cursor::new(json5);
        let archivist: ArchivistConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(archivist, ArchivistConfig::Default);
    }

    #[test]
    fn test_diagnostics_archivist_low_mem() {
        let json5 = r#""low-mem""#;
        let mut cursor = std::io::Cursor::new(json5);
        let archivist: ArchivistConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(archivist, ArchivistConfig::LowMem);
    }

    #[test]
    fn test_diagnostics_missing() {
        let json5 = r#"
        {
          build_type: "eng",
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                archivist: None,
                archivist_pipelines: Vec::new(),
                additional_serial_log_components: Vec::new(),
                sampler: SamplerConfig::default(),
                memory_monitor: MemoryMonitorConfig::default(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_diagnostics_empty() {
        let json5 = r#"
        {
          build_type: "eng",
          diagnostics: {}
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                archivist: None,
                archivist_pipelines: Vec::new(),
                additional_serial_log_components: Vec::new(),
                sampler: SamplerConfig::default(),
                memory_monitor: MemoryMonitorConfig::default(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_diagnostics_with_additional_log_tags() {
        let json5 = r#"
        {
          build_type: "eng",
          diagnostics: {
            additional_serial_log_components: [
                "/foo",
            ]
          }
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                archivist: None,
                archivist_pipelines: Vec::new(),
                additional_serial_log_components: vec!["/foo".to_string()],
                sampler: SamplerConfig::default(),
                memory_monitor: MemoryMonitorConfig::default(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_diagnostics_with_component_log_initial_interests() {
        let json5 = r#"
        {
          build_type: "eng",
          diagnostics: {
            component_log_initial_interests: [
                {
                    component: "fuchsia-boot:///driver_host#meta/driver_host.cm",
                    log_severity: "debug",
                },
                {
                    component: "fuchsia-pkg://fuchsia.com/trace_manager#meta/trace_manager.cm",
                    log_severity: "error",
                },
                {
                    component: "/bootstrap/driver_manager",
                    log_severity: "fatal",
                },
            ]
          }
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                component_log_initial_interests: vec![
                    ComponentInitialInterest {
                        component: UrlOrMoniker::Url(
                            "fuchsia-boot:///driver_host#meta/driver_host.cm".to_string()
                        ),
                        log_severity: LogSeverity::Debug,
                    },
                    ComponentInitialInterest {
                        component: UrlOrMoniker::Url(
                            "fuchsia-pkg://fuchsia.com/trace_manager#meta/trace_manager.cm"
                                .to_string()
                        ),
                        log_severity: LogSeverity::Error,
                    },
                    ComponentInitialInterest {
                        component: UrlOrMoniker::Moniker("/bootstrap/driver_manager".to_string()),
                        log_severity: LogSeverity::Fatal,
                    },
                ],
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_serialize_component_log_initial_interests() {
        assert_eq!(
            serde_json::to_string(&ComponentInitialInterest {
                component: UrlOrMoniker::Url(
                    "fuchsia-boot:///driver_host#meta/driver_host.cm".to_string()
                ),
                log_severity: LogSeverity::Debug,
            })
            .unwrap(),
            "\"fuchsia-boot:///driver_host#meta/driver_host.cm:debug\""
        );
        assert_eq!(
            serde_json::to_string(&ComponentInitialInterest {
                component: UrlOrMoniker::Moniker("/bootstrap/driver_manager".to_string()),
                log_severity: LogSeverity::Fatal,
            })
            .unwrap(),
            "\"/bootstrap/driver_manager:fatal\"",
        );
    }
}
