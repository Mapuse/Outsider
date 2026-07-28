use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::RwLock;
use anyhow::{Result, Context, anyhow};
use serde::Deserialize;
use crate::utils::ui::UserInterface;

const TOOL_PYTHON3: &str = "python3";
const PLUGIN_CONFIG_FILE: &str = "etc/ous/p.desc";
const PATH_PLUGINS: &str = "var/lib/ous/plugins";

// ── TOML plugin configuration ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct PluginConfig {
    #[serde(rename = "plugin")]
    pub plugins: HashMap<String, PluginEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginEntry {
    pub name: String,
    pub path: String,
    #[serde(flatten)]
    pub aliases: HashMap<String, String>,
}

// ── PythonPlugin ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PythonPlugin {
    name: String,
    pub path: PathBuf,
    pub aliases: HashMap<String, String>,
}

impl PythonPlugin {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(anyhow!("Plugin file not found: {:?}", path));
        }
        let path_str = path.to_string_lossy();
        let check_script = format!(
            "compile(open('{}').read(), '{}', 'exec')",
            path_str.replace('\'', "\\'"),
            path_str.replace('\'', "\\'"),
        );
        let output = Command::new(TOOL_PYTHON3)
            .arg("-c")
            .arg(&check_script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("Failed to run python3 for syntax check of {:?}", path))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Python syntax error in {:?}: {}", path, stderr.trim());
        }
        let file_stem = path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Ok(Self {
            name: file_stem,
            path: path.to_path_buf(),
            aliases: HashMap::new(),
        })
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn path(&self) -> &Path { &self.path }

    pub fn run_hook(&self, event: &PluginEvent) -> Result<PluginResult> {
        let event_json = serde_json::to_string(event)
            .with_context(|| "Failed to serialize plugin event")?;
        let path_str = self.path.to_string_lossy();
        let escaped_event = event_json.replace('\'', "\\'");
        let script = format!(
            r#"import json, sys; MCX_EVENT = json.loads('{}'); exec(open('{}').read())"#,
            escaped_event,
            path_str.replace('\'', "\\'"),
        );
        let output = Command::new(TOOL_PYTHON3)
            .arg("-c")
            .arg(&script)
            .current_dir(self.path.parent().unwrap_or(Path::new(".")))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(anyhow!("Plugin '{}' failed: {}", self.name, stderr.trim()));
        }
        let result = serde_json::from_str::<PluginResult>(&stdout)
            .unwrap_or(PluginResult {
                success: output.status.success(),
                message: Some(stdout.trim().to_string()),
            });
        Ok(result)
    }

    pub fn run_alias(&self, alias_name: &str) -> Result<PluginResult> {
        let cmd = self.aliases.get(alias_name)
            .ok_or_else(|| anyhow!("Alias '{}' not found in plugin '{}'", alias_name, self.name))?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(self.path.parent().unwrap_or(Path::new(".")))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok(PluginResult {
            success: output.status.success(),
            message: if stderr.is_empty() { Some(stdout.trim().to_string()) } else { Some(stderr.trim().to_string()) },
        })
    }
}

// ── Plugin event/result ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct PluginEvent {
    pub hook: String,
    pub package: Option<String>,
    pub version: Option<String>,
    pub source: Option<String>,
    pub build_type: Option<String>,
    pub work_dir: Option<String>,
    pub root_dir: Option<String>,
    pub output_path: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginResult {
    pub success: bool,
    pub message: Option<String>,
}

// ── Plugin hooks ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginHook {
    PreFetch,
    PostFetch,
    PreBuild,
    PostBuild,
    PreInstall,
    PostInstall,
    PreHash,
    PostHash,
    PreMetadata,
    PostMetadata,
    PreArchive,
    PostArchive,
}

impl PluginHook {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreFetch => "pre-fetch",
            Self::PostFetch => "post-fetch",
            Self::PreBuild => "pre-build",
            Self::PostBuild => "post-build",
            Self::PreInstall => "pre-install",
            Self::PostInstall => "post-install",
            Self::PreHash => "pre-hash",
            Self::PostHash => "post-hash",
            Self::PreMetadata => "pre-metadata",
            Self::PostMetadata => "post-metadata",
            Self::PreArchive => "pre-archive",
            Self::PostArchive => "post-archive",
        }
    }
}

pub const ALL_HOOKS: &[PluginHook] = &[
    PluginHook::PreFetch, PluginHook::PostFetch,
    PluginHook::PreBuild, PluginHook::PostBuild,
    PluginHook::PreInstall, PluginHook::PostInstall,
    PluginHook::PreHash, PluginHook::PostHash,
    PluginHook::PreMetadata, PluginHook::PostMetadata,
    PluginHook::PreArchive, PluginHook::PostArchive,
];

// ── PluginManager ──────────────────────────────────────────────────────────

struct PluginManagerInner {
    plugins: Vec<PythonPlugin>,
    loaded_paths: HashMap<PathBuf, usize>,
    hook_map: HashMap<PluginHook, Vec<usize>>,
}

pub struct PluginManager {
    inner: RwLock<PluginManagerInner>,
}

impl PluginManager {
    pub fn new(root: &Path) -> Self {
        let mgr = Self {
            inner: RwLock::new(PluginManagerInner {
                plugins: Vec::new(),
                loaded_paths: HashMap::new(),
                hook_map: HashMap::new(),
            }),
        };
        let _ = mgr.load_all(root);
        mgr
    }

    fn load_all(&self, root: &Path) -> Result<()> {
        let config_path = root.join(PLUGIN_CONFIG_FILE);
        if config_path.exists() {
            self.load_config(root)
        } else {
            self.scan_plugins_dir(root)
        }
    }

    fn load_config(&self, root: &Path) -> Result<()> {
        let config_path = root.join(PLUGIN_CONFIG_FILE);
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read plugin config: {:?}", config_path))?;
        let config: PluginConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse plugin config: {:?}", config_path))?;
        let mut loaded = Vec::new();
        let mut loaded_paths = HashMap::new();
        let mut hook_map: HashMap<PluginHook, Vec<usize>> = HashMap::new();
        for (_key, entry) in &config.plugins {
            let path = Self::resolve_path(&entry.path);
            match PythonPlugin::load(&path) {
                Ok(plugin) => {
                    let mut plugin = plugin.with_name(entry.name.clone());
                    plugin.aliases = entry.aliases.clone();
                    let idx = loaded.len();
                    loaded_paths.insert(plugin.path.clone(), idx);
                    loaded.push(plugin);
                    for hook in ALL_HOOKS.iter() {
                        hook_map.entry(*hook).or_default().push(idx);
                    }
                }
                Err(e) => {
                    UserInterface::warning(&format!("Failed to load plugin '{}' from {:?}: {}", entry.name, path, e));
                }
            }
        }
        let mut inner = self.inner.write().unwrap();
        inner.plugins = loaded;
        inner.loaded_paths = loaded_paths;
        inner.hook_map = hook_map;
        Ok(())
    }

    fn scan_plugins_dir(&self, root: &Path) -> Result<()> {
        let plugins_dir = root.join(PATH_PLUGINS);
        if !plugins_dir.exists() {
            return Ok(());
        }
        let mut loaded = Vec::new();
        let mut loaded_paths = HashMap::new();
        let mut hook_map: HashMap<PluginHook, Vec<usize>> = HashMap::new();
        if let Ok(entries) = fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "py").unwrap_or(false) {
                    if let Ok(plugin) = PythonPlugin::load(&path) {
                        let idx = loaded.len();
                        loaded_paths.insert(plugin.path.clone(), idx);
                        loaded.push(plugin);
                        for hook in ALL_HOOKS.iter() {
                            hook_map.entry(*hook).or_default().push(idx);
                        }
                    }
                }
            }
        }
        let mut inner = self.inner.write().unwrap();
        inner.plugins = loaded;
        inner.loaded_paths = loaded_paths;
        inner.hook_map = hook_map;
        Ok(())
    }

    fn resolve_path(path_str: &str) -> PathBuf {
        if path_str.starts_with("~/") {
            if let Some(home) = std::env::var("HOME").ok() {
                return PathBuf::from(home).join(&path_str[2..]);
            }
        }
        PathBuf::from(path_str)
    }

    pub fn reload(&self, root: &Path) {
        let _ = self.load_all(root);
    }

    pub fn reload_from_config(&self, root: &Path) -> Result<usize> {
        let config_path = root.join(PLUGIN_CONFIG_FILE);
        if !config_path.exists() {
            anyhow::bail!("Plugin config not found: {:?}", config_path);
        }
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read plugin config: {:?}", config_path))?;
        let config: PluginConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse plugin config: {:?}", config_path))?;
        let mut new_count = 0;
        let mut inner = self.inner.write().unwrap();
        for (_key, entry) in &config.plugins {
            let path = Self::resolve_path(&entry.path);
            if inner.loaded_paths.contains_key(&path) {
                continue;
            }
            match PythonPlugin::load(&path) {
                Ok(plugin) => {
                    let mut plugin = plugin.with_name(entry.name.clone());
                    plugin.aliases = entry.aliases.clone();
                    let idx = inner.plugins.len();
                    inner.loaded_paths.insert(plugin.path.clone(), idx);
                    inner.plugins.push(plugin);
                    for hook in ALL_HOOKS.iter() {
                        inner.hook_map.entry(*hook).or_default().push(idx);
                    }
                    new_count += 1;
                }
                Err(e) => {
                    UserInterface::warning(&format!("Failed to load plugin '{}' from {:?}: {}", entry.name, path, e));
                }
            }
        }
        Ok(new_count)
    }

    pub fn list(&self) -> Vec<PythonPlugin> {
        let inner = self.inner.read().unwrap();
        inner.plugins.clone()
    }

    pub fn find(&self, name: &str) -> Option<PythonPlugin> {
        let inner = self.inner.read().unwrap();
        inner.plugins.iter().find(|p| p.name() == name).cloned()
    }

    pub fn fire_hook(&self, hook: PluginHook, event: &PluginEvent) {
        let indices: Vec<usize>;
        {
            let inner = self.inner.read().unwrap();
            indices = inner.hook_map.get(&hook).cloned().unwrap_or_default();
        }
        for idx in indices {
            if let Ok(inner) = self.inner.read() {
                if let Some(plugin) = inner.plugins.get(idx) {
                    match plugin.run_hook(event) {
                        Ok(result) => {
                            if !result.success {
                                UserInterface::error(&format!("Plugin '{}' failed: {}", plugin.name(),
                                    result.message.as_deref().unwrap_or("unknown error")));
                            }
                        }
                        Err(e) => {
                            UserInterface::error(&format!("Plugin '{}' error: {}", plugin.name(), e));
                        }
                    }
                }
            }
        }
    }
}

use serde::Serialize;
