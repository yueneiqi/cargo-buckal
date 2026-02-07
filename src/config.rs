use std::collections::BTreeSet as Set;
use std::collections::HashMap;
use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::DocumentMut;

use crate::{
    buckal_warn,
    utils::{UnwrapOrExit, get_buck2_root},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_buck2_binary")]
    pub buck2_binary: String,
}

fn default_buck2_binary() -> String {
    "buck2".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            buck2_binary: default_buck2_binary(),
        }
    }
}

impl Config {
    /// Load configuration from ~/.config/buckal/config.toml
    pub fn load() -> Self {
        let config_path = Self::config_path();

        if !config_path.exists() {
            return Self::default();
        }

        match fs::read_to_string(&config_path) {
            Ok(content) => match toml::from_str::<Config>(&content) {
                Ok(config) => config,
                Err(_) => {
                    eprintln!(
                        "Warning: Failed to parse config file at {}, using defaults",
                        config_path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => {
                eprintln!(
                    "Warning: Failed to read config file at {}, using defaults",
                    config_path.display()
                );
                Self::default()
            }
        }
    }

    /// Get the configuration file path
    pub fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("buckal")
            .join("config.toml")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchEntry {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatchConfig {
    #[serde(default)]
    pub version: HashMap<String, PatchEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub inherit_workspace_deps: bool,
    pub align_cells: bool,
    pub ignore_tests: bool,
    pub patch_fields: Set<String>,
    pub patch: PatchConfig,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            inherit_workspace_deps: false,
            align_cells: false,
            ignore_tests: true,
            patch_fields: Set::new(),
            patch: PatchConfig::default(),
        }
    }
}

impl RepoConfig {
    pub fn load() -> Self {
        let repo_config_path = Self::repo_config_path();

        if !repo_config_path.exists() {
            return Self::default();
        }

        match fs::read_to_string(&repo_config_path) {
            Ok(content) => match toml::from_str::<RepoConfig>(&content) {
                Ok(config) => config,
                Err(_) => {
                    buckal_warn!(
                        "Failed to parse repo config file at {}, using defaults",
                        repo_config_path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => {
                buckal_warn!(
                    "Failed to read repo config file at {}, using defaults",
                    repo_config_path.display()
                );
                Self::default()
            }
        }
    }

    pub fn repo_config_path() -> PathBuf {
        let buck2_root = get_buck2_root().unwrap_or_exit();
        buck2_root.join("buckal.toml").into()
    }

    pub fn save_patch_entry(dep_name: &str, from_version: &str, to_version: &str) -> Result<()> {
        let path = Self::repo_config_path();
        let content = if path.exists() {
            fs::read_to_string(&path).context("failed to read buckal.toml")?
        } else {
            String::new()
        };

        let mut doc = content.parse::<DocumentMut>().context("failed to parse buckal.toml")?;

        // Ensure [patch] table exists
        if !doc.contains_key("patch") {
            doc.insert("patch", toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let patch_table = doc["patch"]
            .as_table_mut()
            .context("[patch] is not a table")?;

        // Ensure [patch.version] table exists
        if !patch_table.contains_key("version") {
            patch_table.insert("version", toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let version_table = patch_table["version"]
            .as_table_mut()
            .context("[patch.version] is not a table")?;

        // Insert or update the entry for this dep
        let mut entry = toml_edit::InlineTable::new();
        entry.insert("from", toml_edit::Value::from(from_version));
        entry.insert("to", toml_edit::Value::from(to_version));
        version_table.insert(dep_name, toml_edit::Item::Value(toml_edit::Value::InlineTable(entry)));

        fs::write(&path, doc.to_string()).context("failed to write buckal.toml")?;
        Ok(())
    }
}
