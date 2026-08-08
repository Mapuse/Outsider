use serde::{Deserialize, Serialize};

fn default_false() -> bool { false }
fn default_log_level() -> String { "info".into() }
fn default_log_file() -> String { "/var/log/ous.md".into() }
fn default_zstd_level() -> u32 { 3 }
fn default_target_arch() -> String { "x86_64-unknown-linux-musl".into() }
fn default_base_url() -> String { "https://raw.codeberg.org/Cudane/Repository".into() }
fn default_empty() -> String { String::new() }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub general: GeneralConfig,
    pub build: BuildConfig,
    pub repository: RepositoryConfig,
    pub gpg: GpgConfig,
    pub python: PythonConfig,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub log_level: String,
    pub log_file: String,
    pub quiet: bool,
    pub debug: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            log_file: default_log_file(),
            quiet: default_false(),
            debug: default_false(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    pub zstd_level: u32,
    pub target_arch: String,
    pub rust_flags: String,
    pub parallel: bool,
    pub jobs: usize,
    pub force: bool,
    pub clean: bool,
    pub keep_src: bool,
    pub no_auto: bool,
    pub hash_type: String,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            zstd_level: default_zstd_level(),
            target_arch: default_target_arch(),
            rust_flags: String::new(),
            parallel: default_false(),
            jobs: 1,
            force: default_false(),
            clean: default_false(),
            keep_src: default_false(),
            no_auto: default_false(),
            hash_type: "sha256".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RepositoryConfig {
    pub base_url: String,
    pub arch: String,
}

impl Default for RepositoryConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            arch: default_target_arch(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GpgConfig {
    pub key_id: String,
    pub sign_index: bool,
    pub sign_packages: bool,
}

impl Default for GpgConfig {
    fn default() -> Self {
        Self {
            key_id: default_empty(),
            sign_index: default_false(),
            sign_packages: default_false(),
        }
    }
}

pub use cps::PythonConfig;
