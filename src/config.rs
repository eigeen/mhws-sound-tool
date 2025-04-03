use std::sync::LazyLock;

use eyre::Context;
use log::{error, warn};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{ffmpeg::FFmpegCli, wwise::WwiseConsole};

const CONFIG_PATH: &str = "config.toml";
static GLOBAL_CONFIG: LazyLock<Mutex<Config>> = LazyLock::new(|| Mutex::new(Config::init_load()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: i32,
    #[serde(default)]
    pub bin: Vec<BinConfig>,
}

impl Config {
    fn init_load() -> Config {
        let mut config = load_config(CONFIG_PATH);
        if let Err(e) = config.initialize() {
            warn!("Failed to initialize config: {}", e);
        }
        config
    }

    pub fn initialize(&mut self) -> eyre::Result<()> {
        if self.get_bin_config("ffmpeg").is_none() {
            if let Ok(ffmpeg) = FFmpegCli::new() {
                self.set_bin_config("ffmpeg", ffmpeg.program_path().to_string_lossy().as_ref());
            }
        }
        if self.get_bin_config("WwiseConsole").is_none() {
            if let Ok(wwise_console) = WwiseConsole::new() {
                self.set_bin_config(
                    "WwiseConsole",
                    wwise_console.program_path().to_string_lossy().as_ref(),
                );
            }
        }
        Ok(())
    }

    pub fn global() -> &'static Mutex<Config> {
        &GLOBAL_CONFIG
    }

    pub fn get_bin_config(&self, name: &str) -> Option<&BinConfig> {
        self.bin.iter().find(|b| b.name == name)
    }

    pub fn get_bin_config_mut(&mut self, name: &str) -> Option<&mut BinConfig> {
        self.bin.iter_mut().find(|b| b.name == name)
    }

    pub fn set_bin_config(&mut self, name: &str, path: &str) {
        if let Some(bin) = self.get_bin_config_mut(name) {
            bin.path = path.to_string();
        } else {
            self.bin.push(BinConfig {
                name: name.to_string(),
                path: path.to_string(),
                params: vec![],
            });
        }
    }

    pub fn try_save(&self) -> eyre::Result<()> {
        let config_string = toml::to_string_pretty(self).context("Failed to serialize config")?;
        std::fs::write(CONFIG_PATH, config_string).context("Failed to write config file")?;
        Ok(())
    }

    pub fn save(&self) {
        if let Err(e) = self.try_save() {
            error!("Failed to save config: {}", e);
        }
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
