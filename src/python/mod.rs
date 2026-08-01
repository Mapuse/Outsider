pub mod theme;
pub mod plugin;
pub mod tui;

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use pyo3::prelude::*;
use crate::config::schema::PythonConfig;
use crate::utils::ui::UserInterface;

static INIT: Once = Once::new();
static INIT_FAILED: AtomicBool = AtomicBool::new(false);

pub struct PythonEngine {
    pub theme: Option<theme::ThemeEngine>,
    pub tui: Option<tui::TuiEngine>,
    pub plugins: plugin::PluginManager,
    pub tui_mode: bool,
}

impl PythonEngine {
    pub fn new(cfg: &PythonConfig) -> Self {
        if !cfg.enabled {
            return Self { theme: None, tui: None, plugins: plugin::PluginManager::new(), tui_mode: false };
        }
        INIT.call_once(|| {
            let ok = std::panic::catch_unwind(|| {
                pyo3::prepare_freethreaded_python();
            }).is_ok();
            if !ok {
                INIT_FAILED.store(true, Ordering::SeqCst);
            }
        });
        if INIT_FAILED.load(Ordering::SeqCst) {
            UserInterface::warning("python engine unavailable, falling back to native");
            return Self { theme: None, tui: None, plugins: plugin::PluginManager::new(), tui_mode: false };
        }
        if !cfg.venv_path.is_empty() {
            activate_venv(&cfg.venv_path);
        }
        let theme = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            theme::ThemeEngine::load(cfg)
        }))
        .unwrap_or_else(|e| {
            UserInterface::warning(&format!("python theme failed to load: {:?}", e));
            None
        });
        let tui = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tui::TuiEngine::load(cfg)
        }))
        .unwrap_or_else(|e| {
            UserInterface::warning(&format!("python tui failed to load: {:?}", e));
            None
        });
        let mut plugins = plugin::PluginManager::new();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plugins.load_all(cfg);
        }));
        Self { theme, tui, plugins, tui_mode: cfg.tui_mode }
    }
}

pub fn expand_tilde(path: &str) -> String {
    if path.starts_with('~')
        && let Ok(home) = std::env::var("HOME") {
            return path.replacen('~', &home, 1);
        }
    path.to_string()
}

fn activate_venv(path_str: &str) {
    let venv = std::path::PathBuf::from(expand_tilde(path_str));
    if !venv.exists() {
        UserInterface::warning(&format!("venv not found: {}", venv.display()));
        return;
    }
    let _ = Python::with_gil(|py| -> PyResult<()> {
        let sys = py.import("sys")?;
        let sys_path = sys.getattr("path")?;
        let plat = std::env::consts::OS;
        let candidates: Vec<std::path::PathBuf> = if plat == "windows" {
            vec![venv.join("Lib").join("site-packages")]
        } else {
            let mut v = Vec::new();
            if let Ok(entries) = std::fs::read_dir(venv.join("lib")) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("python") {
                        v.push(entry.path().join("site-packages"));
                    }
                }
            }
            v.push(venv.join("Lib").join("site-packages"));
            v
        };
        for p in &candidates {
            if p.exists() {
                sys_path.call_method1("insert", (0, p.to_str().unwrap_or_default()))?;
                UserInterface::info(&format!("activated venv: {} (site-packages: {})", venv.display(), p.display()));
                return Ok(());
            }
        }
        UserInterface::warning(&format!("venv site-packages not found in: {}", venv.display()));
        Ok(())
    });
}
