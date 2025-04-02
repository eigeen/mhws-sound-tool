use std::sync::LazyLock;

use eyre::Context;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

static GLOBAL_CONFIG: LazyLock<Mutex<Config>> = LazyLock::new(|| Mutex::new(Config::init_load()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: i32,
    #[serde(default)]
    pub bin: Vec<BinConfig>,
}

impl Config {
    pub fn global() -> &'static Mutex<Config> {
        &GLOBAL_CONFIG
    }

    fn init_load() -> Config {
        let config_path = "config.toml";
        load_config(config_path)
    }

    pub fn get_bin_config(&self, name: &str) -> Option<&BinConfig> {
        self.bin.iter().find(|b| b.name == name)
    }

    pub fn get_bin_config_mut(&mut self, name: &str) -> Option<&mut BinConfig> {
        self.bin.iter_mut().find(|b| b.name == name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinConfig {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub params: Vec<String>,
}

/// Load the config from a file, or use the default config if it doesn't exist.
fn load_config(path: &str) -> Config {
    if let Ok(config) = load_config_from_file(path) {
        config
    } else {
        default_config()
    }
}

fn load_config_from_file(path: &str) -> eyre::Result<Config> {
    let config_string = std::fs::read_to_string(path).context("Failed to read config file")?;
    // dynamically deserialize, version check
    let config: serde_json::Value = toml::from_str(&config_string)?;
    let version = config
        .get("version")
        .ok_or(eyre::eyre!("No version field in config"))?;
    let version = version
        .as_i64()
        .ok_or(eyre::eyre!("Version field is not an integer"))?;
    if version != 1 {
        return Err(eyre::eyre!("Unsupported config version: {}", version));
    }
    // deserialize the config
    let config: Config = toml::from_str(&config_string)?;
    Ok(config)
}

fn default_config() -> Config {
    Config {
        version: 1,
        bin: vec![],
    }
}
