use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock, Once};
use pyo3::prelude::*;
use crate::config::schema::PythonConfig;
use crate::utils::ui::UserInterface;
use super::expand_tilde;
use std::fs;

#[derive(serde::Deserialize)]
struct TuiDescConfig {
    #[serde(rename = "tui")]
    tuis: HashMap<String, TuiDescEntry>,
}

#[derive(serde::Deserialize)]
struct TuiDescEntry {
    name: String,
    path: String,
    description: Option<String>,
}

pub struct TuiEngine {
    module: PyObject,
}

impl TuiEngine {
    pub fn load(cfg: &PythonConfig) -> Option<Self> {
        if cfg.tui.is_empty() { return None; }
        let path = expand_tilde(&cfg.tui);
        let std_path = std::path::PathBuf::from(&path);
        if !std_path.exists() {
            UserInterface::warning(&format!("tui file not found: {}", path));
            return None;
        }
        let parent = std_path.parent()?;
        let file_stem = std_path.file_stem()?.to_str()?;
        let parent_str = parent.to_str()?.to_string();
        let file_stem = file_stem.to_string();
        let result: PyResult<Self> = Python::with_gil(|py| {
            let sys = py.import("sys")?;
            sys.getattr("path")?.call_method1("insert", (0, &parent_str))?;
            let module = py.import(&file_stem)?.into();
            Ok(Self { module })
        });
        match result {
            Ok(engine) => {
                UserInterface::info(&format!("loaded tui: {}", path));
                Some(engine)
            }
            Err(e) => {
                UserInterface::error(&format!("failed to load tui {}: {}", path, e));
                None
            }
        }
    }

    pub fn has_run(&self) -> bool {
        Python::with_gil(|py| {
            self.module.bind(py).hasattr("run").unwrap_or(false)
        })
    }

    pub fn run(&self) -> bool {
        Python::with_gil(|py| {
            match self.module.call_method0(py, "run") {
                Ok(_) => true,
                Err(e) => {
                    UserInterface::error(&format!("python TUI exited: {}", e));
                    false
                }
            }
        })
    }
}

fn tui_desc_candidates() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(std::path::PathBuf::from(home).join(".config/ous/t.desc"));
    }
    candidates.push(std::path::PathBuf::from("/etc/ous/t.desc"));
    candidates.push(std::path::PathBuf::from("./t.desc"));
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("t.desc"));
    }
    candidates
}

fn ensure_tui_desc_loaded() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for path in tui_desc_candidates() {
            if path.exists()
                && let Ok(content) = fs::read_to_string(&path)
                && let Ok(config) = toml::from_str::<TuiDescConfig>(&content)
            {
                for (id, entry) in config.tuis {
                    let expanded = expand_tilde(&entry.path);
                    let dest = Path::new(&expanded);
                    let description = entry.description.unwrap_or_default();
                    TuiEngine::register_desc(&id, &entry.name, dest, &description);
                }
                UserInterface::info(&format!("loaded tuis from t.desc: {}", path.display()));
                return;
            }
        }
    });
}

#[derive(Debug, Clone)]
pub struct TuiEntry {
    pub name: String,
    pub path: String,
    pub description: String,
}

fn tui_registry() -> &'static Mutex<HashMap<String, TuiEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, TuiEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl TuiEngine {
    pub fn list() -> Vec<TuiEntry> {
        ensure_tui_desc_loaded();
        tui_registry().lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    pub fn by_name(name: &str) -> Option<TuiEntry> {
        ensure_tui_desc_loaded();
        let registry = tui_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.get(name).cloned()
            .or_else(|| registry.values().find(|e| e.name == name).cloned())
    }

    pub fn apply(entry: &TuiEntry) -> Result<String, String> {
        ensure_tui_desc_loaded();
        let path = expand_tilde(&entry.path);
        let std_path = std::path::PathBuf::from(&path);
        if !std_path.exists() {
            return Err(format!("tui file not found: {}", path));
        }
        let output = std::process::Command::new("python3")
            .arg(&path)
            .output()
            .map_err(|e| format!("failed to run tui: {}", e))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    pub fn register(name: &str, dest: &Path) {
        ensure_tui_desc_loaded();
        let mut registry = tui_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.insert(name.to_string(), TuiEntry {
            name: name.to_string(),
            path: dest.to_string_lossy().to_string(),
            description: String::new(),
        });
    }

    pub fn register_desc(name: &str, display_name: &str, dest: &Path, description: &str) {
        let mut registry = tui_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.insert(name.to_string(), TuiEntry {
            name: display_name.to_string(),
            path: dest.to_string_lossy().to_string(),
            description: description.to_string(),
        });
    }

    pub fn unregister(name: &str) {
        let mut registry = tui_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.remove(name);
        registry.retain(|_, e| e.name != name);
    }
}
