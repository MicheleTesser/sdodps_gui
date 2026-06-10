use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

const DEFAULT_CONFIG_PATH: &str = "sdodps_gui.toml";
const DEFAULT_DBC_PATH: &str = "dbc/can2.dbc";
const DEFAULT_SOCKETCAN: &str = "can0";
const DEFAULT_DBCC_PATH: &str = "dbc/dbcc/dbcc";

#[derive(Debug, Parser)]
#[command(author, version, about = "Ratatui frontend for SDO/DPS over SocketCAN")]
pub struct CliArgs {
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub dbc: Option<PathBuf>,
    #[arg(long)]
    pub can: Option<String>,
    #[arg(long)]
    pub dbcc: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    pub dbc_path: Option<PathBuf>,
    pub socketcan: Option<String>,
    pub dbcc_path: Option<PathBuf>,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            dbc_path: Some(PathBuf::from(DEFAULT_DBC_PATH)),
            socketcan: Some(DEFAULT_SOCKETCAN.to_string()),
            dbcc_path: default_dbcc_path(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub config_path: PathBuf,
    pub dbc_path: PathBuf,
    pub socketcan: String,
    pub dbcc_path: Option<PathBuf>,
}

impl RuntimeConfig {
    pub fn load() -> Result<Self> {
        let args = CliArgs::parse();
        let config_path = args
            .config
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));

        let file_config = if config_path.exists() {
            load_file_config(&config_path)?
        } else {
            let default = FileConfig::default();
            write_default_config(&config_path, &default)?;
            default
        };

        Ok(Self {
            dbc_path: args
                .dbc
                .or(file_config.dbc_path)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_DBC_PATH)),
            socketcan: args
                .can
                .or(file_config.socketcan)
                .unwrap_or_else(|| DEFAULT_SOCKETCAN.to_string()),
            dbcc_path: args
                .dbcc
                .or(file_config.dbcc_path)
                .or_else(default_dbcc_path),
            config_path,
        })
    }
}

fn load_file_config(path: &Path) -> Result<FileConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse config file {}", path.display()))
}

fn write_default_config(path: &Path, file_config: &FileConfig) -> Result<()> {
    let raw = toml::to_string_pretty(file_config).context("failed to serialize default config")?;
    fs::write(path, raw)
        .with_context(|| format!("failed to write default config file {}", path.display()))
}

fn default_dbcc_path() -> Option<PathBuf> {
    let path = PathBuf::from(DEFAULT_DBCC_PATH);
    path.exists().then_some(path)
}
