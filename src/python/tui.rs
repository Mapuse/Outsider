use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;
use pyo3::prelude::*;
use crate::config::schema::PythonConfig;
use super::expand_tilde;

pub struct TuiEngine {
    module: PyObject,
}

impl TuiEngine {
    pub fn load(cfg: &PythonConfig) -> Option<Self> {
        if cfg.tui.is_empty() { return None; }
        let path = expand_tilde(&cfg.tui);
        let std_path = std::path::PathBuf::from(&path);
        if !std_path.exists() {
            eprintln!("ous: tui file not found: {}", path);
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
                eprintln!("ous: loaded tui: {}", path);
                Some(engine)
            }
            Err(e) => {
                eprintln!("ous: failed to load tui {}: {}", path, e);
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
                    eprintln!("ous: python TUI exited: {}", e);
                    false
                }
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct TuiEntry {
    pub name: String,
    pub path: String,
}

fn tui_registry() -> &'static Mutex<HashMap<String, TuiEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, TuiEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl TuiEngine {
    pub fn list() -> Vec<TuiEntry> {
        tui_registry().lock().expect("tui registry lock").values().cloned().collect()
    }

    pub fn by_name(name: &str) -> Option<TuiEntry> {
        tui_registry().lock().expect("tui registry lock").get(name).cloned()
    }

    pub fn apply(entry: &TuiEntry) -> Result<String, String> {
        let _ = entry;
        eprintln!("ous: tui apply not available in backward-compat mode");
        Err("tui apply not available in backward-compat mode".to_string())
    }

    pub fn register(name: &str, dest: &Path) {
        let mut registry = tui_registry().lock().expect("tui registry lock");
        registry.insert(name.to_string(), TuiEntry {
            name: name.to_string(),
            path: dest.to_string_lossy().to_string(),
        });
    }

    pub fn unregister(name: &str) {
        let mut registry = tui_registry().lock().expect("tui registry lock");
        registry.remove(name);
    }
}
