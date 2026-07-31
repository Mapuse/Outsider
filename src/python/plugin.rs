use pyo3::prelude::*;
use pyo3::types::PyDict;
use crate::config::schema::PythonConfig;
use super::expand_tilde;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
}

struct LoadedPlugin {
    name: String,
    module: PyObject,
    hooks: Vec<String>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    pub fn new() -> Self {
        Self { plugins: vec![] }
    }

    pub fn load_all(&mut self, cfg: &PythonConfig) {
        if cfg.plugins.is_empty() { return; }
        let plugins_result: PyResult<Vec<(String, PyObject, Vec<String>)>> = Python::with_gil(|py| {
            let mut loaded = Vec::new();
            let sys_path = py.import("sys")?.getattr("path")?;
            for plugin_path in &cfg.plugins {
                let path = expand_tilde(plugin_path);
                let std_path = std::path::PathBuf::from(&path);
                if !std_path.exists() {
                    eprintln!("ous: plugin not found: {}", path);
                    continue;
                }
                let parent = match std_path.parent() {
                    Some(p) => p.to_str().unwrap_or(".").to_string(),
                    None => { eprintln!("ous: cannot determine parent of {}", path); continue; }
                };
                let file_stem = match std_path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => { eprintln!("ous: cannot determine name of {}", path); continue; }
                };
                let _ = sys_path.call_method1("insert", (0, &parent));
                match load_one_plugin(py, &file_stem) {
                    Some((module, hooks)) => {
                        eprintln!("ous: loaded plugin: {} (hooks: {})", file_stem, hooks.join(", "));
                        loaded.push((file_stem, module, hooks));
                    }
                    None => {
                        eprintln!("ous: plugin {} has no hooks, skipping", file_stem);
                    }
                }
            }
            Ok(loaded)
        });
        for (name, module, hooks) in plugins_result.unwrap_or_default() {
            self.plugins.push(LoadedPlugin { name, module, hooks });
        }
    }

    pub fn fire(&self, event: &str, data: &std::collections::HashMap<String, String>) {
        let _ = Python::with_gil(|py| -> PyResult<()> {
            for plugin in &self.plugins {
                if plugin.hooks.contains(&event.to_string()) {
                    let kwargs = PyDict::new(py);
                    for (k, v) in data {
                        kwargs.set_item(k.as_str(), v.as_str())?;
                    }
                    let _ = plugin.module.call_method(py, event, (), Some(&kwargs));
                }
            }
            Ok(())
        });
    }

    pub fn count(&self) -> usize {
        self.plugins.len()
    }

    pub fn names(&self) -> Vec<String> {
        self.plugins.iter().map(|p| p.name.clone()).collect()
    }
}

fn load_one_plugin(py: Python, file_stem: &str) -> Option<(PyObject, Vec<String>)> {
    let module = py.import(file_stem).ok()?;
    let dir = module.dir().ok()?;
    let mut hooks = Vec::new();
    let builtins = py.import("builtins").ok()?;
    let callable = builtins.getattr("callable").ok()?;
    for item in dir.iter() {
        if let Ok(name) = item.extract::<String>() {
            if name.starts_with('_') { continue; }
            if let Ok(attr) = module.getattr(name.as_str())
                && callable.call1((attr,)).and_then(|r| r.extract::<bool>()).unwrap_or(false) {
                    hooks.push(name);
                }
        }
    }
    if hooks.is_empty() { return None; }
    Some((module.into(), hooks))
}

#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub name: String,
    pub path: String,
    pub aliases: HashMap<String, String>,
}

fn plugin_registry() -> &'static Mutex<HashMap<String, PluginEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, PluginEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl PluginManager {
    pub fn list() -> Vec<PluginEntry> {
        plugin_registry().lock().expect("plugin registry lock").values().cloned().collect()
    }

    pub fn by_alias(alias: &str) -> Option<(PluginEntry, String)> {
        let registry = plugin_registry().lock().expect("plugin registry lock");
        for entry in registry.values() {
            if let Some(cmd) = entry.aliases.get(alias) {
                return Some((entry.clone(), cmd.clone()));
            }
        }
        None
    }

    pub fn run(entry: &PluginEntry, func: &str, args: &[String]) -> Result<String, String> {
        let _ = (entry, func, args);
        eprintln!("ous: plugin run not available in backward-compat mode");
        Err("plugin run not available in backward-compat mode".to_string())
    }

    pub fn register(name: &str, dest: &Path, aliases: &HashMap<String, String>) {
        let mut registry = plugin_registry().lock().expect("plugin registry lock");
        registry.insert(name.to_string(), PluginEntry {
            name: name.to_string(),
            path: dest.to_string_lossy().to_string(),
            aliases: aliases.clone(),
        });
    }

    pub fn unregister(name: &str) {
        let mut registry = plugin_registry().lock().expect("plugin registry lock");
        registry.remove(name);
    }

    pub fn by_name(name: &str) -> Option<PluginEntry> {
        plugin_registry().lock().expect("plugin registry lock").get(name).cloned()
    }
}
