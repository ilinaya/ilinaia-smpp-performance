use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusmpp::values::{Npi, Ton};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub smpp: SmppConfig,
    pub message: MessageConfig,
    pub load: LoadConfig,
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .with_context(|| format!("Unable to read config from {}", path.display()))?;
        let config: Config = toml::from_str(&data).context("Failed to parse TOML configuration")?;

        if config.load.binds == 0 {
            tracing::warn!("Configured bind count is 0. No traffic will be generated.");
        }

        Ok(config)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SmppConfig {
    pub host: String,
    pub port: u16,
    pub system_id: String,
    #[serde(default)]
    pub system_type: Option<String>,
    pub password: String,
}

impl SmppConfig {
    pub fn connection_uri(&self) -> String {
        format!("smpp://{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageConfig {
    pub source_addr: String,
    #[serde(default)]
    pub source_ton: u8,
    #[serde(default)]
    pub source_npi: u8,
    pub destination_addr: String,
    #[serde(default)]
    pub destination_ton: u8,
    #[serde(default)]
    pub destination_npi: u8,
    pub body: String,
    #[serde(default)]
    pub service_type: Option<String>,
}

impl MessageConfig {
    pub fn source_ton(&self) -> Ton {
        Ton::from(self.source_ton)
    }

    pub fn source_npi(&self) -> Npi {
        Npi::from(self.source_npi)
    }

    pub fn destination_ton(&self) -> Ton {
        Ton::from(self.destination_ton)
    }

    pub fn destination_npi(&self) -> Npi {
        Npi::from(self.destination_npi)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadConfig {
    #[serde(default = "default_binds")]
    pub binds: usize,
    #[serde(default = "default_max_tps")]
    pub max_tps_per_bind: u32,
    #[serde(default = "default_inflight")]
    pub inflight_per_bind: usize,
}

impl LoadConfig {
    pub fn max_tps_per_bind(&self) -> u32 {
        if self.max_tps_per_bind == 0 {
            100
        } else {
            self.max_tps_per_bind
        }
    }

    pub fn inflight_per_bind(&self) -> usize {
        if self.inflight_per_bind == 0 {
            default_inflight()
        } else {
            self.inflight_per_bind
        }
    }
}

const fn default_binds() -> usize {
    1
}

const fn default_max_tps() -> u32 {
    100
}

const fn default_inflight() -> usize {
    64
}
