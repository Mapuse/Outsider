use std::fs;
use std::path::PathBuf;

use super::schema::Config;

const CONFIG_FILE: &str = "ous.toml";

fn config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".config/ous"));
    }
    dirs.push(PathBuf::from("/etc/ous"));
    dirs.push(PathBuf::from("."));
    dirs
}

pub fn config_path() -> PathBuf {
    for dir in config_dirs() {
        let p = dir.join(CONFIG_FILE);
        if p.is_file() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".config/ous").join(CONFIG_FILE);
        let _ = fs::create_dir_all(p.parent().expect("path has parent"));
        return p;
    }
    PathBuf::from(CONFIG_FILE)
}

pub fn load() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("ous: error parsing {}: {}", path.display(), e);
                Config::default()
            }
        },
        Err(_) => {
            let cfg = Config::default();
            let _ = save(&cfg);
            cfg
        }
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let toml = toml::to_string_pretty(cfg).unwrap_or_default();
    fs::write(path, toml)
}
