use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock, Once};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use crate::config::schema::PythonConfig;
use crate::utils::ui::UserInterface;
use super::expand_tilde;
use std::fs;

#[derive(serde::Deserialize)]
struct ThemeDescConfig {
    #[serde(rename = "theme")]
    themes: HashMap<String, ThemeDescEntry>,
}

#[derive(serde::Deserialize)]
struct ThemeDescEntry {
    name: String,
    path: String,
    description: Option<String>,
}

pub struct ThemeEngine {
    module: PyObject,
}

#[derive(Debug, Clone, Default)]
pub struct ThemeResult {
    pub lines_above: Vec<String>,
    pub input_prefix: String,
    pub right_prompt: String,
    pub colors: HashMap<String, String>,
    pub extra: HashMap<String, String>,
}

impl ThemeEngine {
    pub fn load(cfg: &PythonConfig) -> Option<Self> {
        if cfg.theme.is_empty() { return None; }
        let path = expand_tilde(&cfg.theme);
        let std_path = std::path::PathBuf::from(&path);
        if !std_path.exists() {
            UserInterface::warning(&format!("theme file not found: {}", path));
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
                UserInterface::info(&format!("loaded theme: {}", path));
                Some(engine)
            }
            Err(e) => {
                UserInterface::error(&format!("failed to load theme {}: {}", path, e));
                None
            }
        }
    }

    pub fn render_prompt(&self, context: &HashMap<String, String>) -> ThemeResult {
        let default = ThemeResult::default_prompt(context);
        let result: PyResult<ThemeResult> = Python::with_gil(|py| {
            let kwargs = PyDict::new(py);
            for (k, v) in context {
                kwargs.set_item(k.as_str(), v.as_str())?;
            }
            let val = self.module.call_method(py, "render_prompt", (), Some(&kwargs))?;
            parse_theme_result(py, &val)
        });
        result.unwrap_or(default)
    }

    pub fn render_right_prompt(&self, context: &HashMap<String, String>) -> String {
        let result: PyResult<String> = Python::with_gil(|py| {
            let kwargs = PyDict::new(py);
            for (k, v) in context {
                kwargs.set_item(k.as_str(), v.as_str())?;
            }
            let val = self.module.call_method(py, "render_right_prompt", (), Some(&kwargs))?;
            val.extract::<String>(py)
        });
        result.unwrap_or_default()
    }

    pub fn render_command_summary(&self, context: &HashMap<String, String>) -> String {
        let result: PyResult<String> = Python::with_gil(|py| {
            let kwargs = PyDict::new(py);
            for (k, v) in context {
                kwargs.set_item(k.as_str(), v.as_str())?;
            }
            let val = self.module.call_method(py, "render_command_summary", (), Some(&kwargs))?;
            val.extract::<String>(py)
        });
        result.unwrap_or_default()
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

fn theme_desc_candidates() -> Vec<std::path::PathBuf> {
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

fn ensure_tdesc_loaded() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for path in theme_desc_candidates() {
            if path.exists()
                && let Ok(content) = fs::read_to_string(&path)
                && let Ok(config) = toml::from_str::<ThemeDescConfig>(&content)
            {
                for (id, entry) in config.themes {
                    let expanded = expand_tilde(&entry.path);
                    let dest = Path::new(&expanded);
                    let description = entry.description.unwrap_or_default();
                    ThemeEngine::register_desc(&id, &entry.name, dest, &description);
                }
                UserInterface::info(&format!("loaded themes from t.desc: {}", path.display()));
                return;
            }
        }
    });
}

fn parse_theme_result(py: Python, val: &PyObject) -> PyResult<ThemeResult> {
    let mut res = ThemeResult::default();
    let any = val.bind(py);
    if let Ok(dict) = any.downcast::<PyDict>() {
        if let Ok(Some(v)) = dict.get_item("lines_above")
            && let Ok(list) = v.downcast::<pyo3::types::PyList>() {
                res.lines_above = list.iter().filter_map(|x| x.extract().ok()).collect();
            }
        if let Ok(Some(v)) = dict.get_item("input_prefix") { res.input_prefix = v.extract().unwrap_or_default(); }
        if let Ok(Some(v)) = dict.get_item("right_prompt") { res.right_prompt = v.extract().unwrap_or_default(); }
        if let Ok(Some(c)) = dict.get_item("colors")
            && let Ok(cd) = c.downcast::<PyDict>() {
                for item in cd.iter() {
                    if let (Ok(key), Ok(val)) = (item.0.extract::<String>(), item.1.extract::<String>()) {
                        res.colors.insert(key, val);
                    }
                }
            }
        for item in dict.iter() {
            if let (Ok(key), Ok(val)) = (item.0.extract::<String>(), item.1.extract::<String>())
                && key != "lines_above" && key != "input_prefix" && key != "right_prompt" && key != "colors" {
                    res.extra.insert(key, val);
                }
        }
    } else if let Ok(s) = any.extract::<String>() {
        res.lines_above = vec![s];
    }
    Ok(res)
}

impl ThemeResult {
    fn default_prompt(context: &HashMap<String, String>) -> Self {
        let cwd = context.get("cwd").map(|s| s.as_str()).unwrap_or("~");
        Self {
            lines_above: vec![],
            input_prefix: format!("{} ❯ ", cwd),
            right_prompt: String::new(),
            colors: HashMap::new(),
            extra: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThemeEntry {
    pub name: String,
    pub path: String,
    pub description: String,
}

fn theme_registry() -> &'static Mutex<HashMap<String, ThemeEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, ThemeEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl ThemeEngine {
    pub fn list() -> Vec<ThemeEntry> {
        ensure_tdesc_loaded();
        theme_registry().lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    pub fn by_name(name: &str) -> Option<ThemeEntry> {
        ensure_tdesc_loaded();
        let registry = theme_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.get(name).cloned()
            .or_else(|| registry.values().find(|e| e.name == name).cloned())
    }

    pub fn apply(entry: &ThemeEntry) -> Result<String, String> {
        ensure_tdesc_loaded();
        let path = expand_tilde(&entry.path);
        let std_path = std::path::PathBuf::from(&path);
        if !std_path.exists() {
            return Err(format!("theme file not found: {}", path));
        }
        let output = std::process::Command::new("python3")
            .arg(&path)
            .output()
            .map_err(|e| format!("failed to run theme: {}", e))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    pub fn register(name: &str, dest: &Path) {
        ensure_tdesc_loaded();
        let mut registry = theme_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.insert(name.to_string(), ThemeEntry {
            name: name.to_string(),
            path: dest.to_string_lossy().to_string(),
            description: String::new(),
        });
    }

    pub fn register_desc(name: &str, display_name: &str, dest: &Path, description: &str) {
        let mut registry = theme_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.insert(name.to_string(), ThemeEntry {
            name: display_name.to_string(),
            path: dest.to_string_lossy().to_string(),
            description: description.to_string(),
        });
    }

    pub fn unregister(name: &str) {
        let mut registry = theme_registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.remove(name);
        registry.retain(|_, e| e.name != name);
    }
}
