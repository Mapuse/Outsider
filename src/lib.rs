use anyhow::{Context, Result, anyhow};
use md5::{Digest as _, Md5};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Read,
    path::Path,
    path::PathBuf,
    process::Command,
};

pub mod config;
pub mod deps;
pub mod upload;
pub mod utils;
use crate::utils::ui::UserInterface;

use std::sync::{Mutex, OnceLock};

/// Write `bytes` to `path` atomically: a temporary sibling file in the same
/// directory is written, fsynced, then renamed over the target so readers
/// never observe a truncated file even if the process dies mid-write.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().map_or_else(
        || "ous.tmp".to_string(),
        |n| n.to_string_lossy().to_string(),
    );
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{}.tmp-{}-{}", name, std::process::id(), nanos));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Parse `OUS_ZSTD_LEVEL`, clamping to the range zstd supports (1..=22).
/// Invalid values fall back to the default (3) with a warning.
pub fn zstd_level_from_env() -> u32 {
    match env::var("OUS_ZSTD_LEVEL") {
        Ok(raw) => match raw.trim().parse::<u32>() {
            Ok(lvl) if (1..=22).contains(&lvl) => lvl,
            _ => {
                UserInterface::warning(&format!(
                    "Invalid OUS_ZSTD_LEVEL '{raw}' — falling back to default level 3"
                ));
                3
            }
        },
        Err(_) => 3,
    }
}

/// Validate a raw version string interpolated into the output filename:
/// rejects empty values, path separators and `..` traversal.
fn validate_version_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.contains('/')
        || value.split('/').any(|c| c == "..")
        || value.contains("..")
    {
        return Err(anyhow!(
            "Invalid {label} '{value}': '/', '..' and empty values are forbidden"
        ));
    }
    Ok(())
}

/// Directory has at least one entry (used to sanity-check resume markers).
fn dir_non_empty(p: &Path) -> bool {
    fs::read_dir(p)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false)
}

/// Canonicalize a raw architecture string to mcx's canonical names:
/// `amd64` → `x86_64`, `arm64` → `aarch64`, `x86_64`/`aarch64`/`native`
/// pass through verbatim. Returns an error for any other value.
pub fn canonical_arch(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok("native".to_string());
    }
    match trimmed {
        "x86_64" | "aarch64" | "native" => Ok(trimmed.to_string()),
        "amd64" => Ok("x86_64".to_string()),
        "arm64" => Ok("aarch64".to_string()),
        _ => Err(anyhow!(
            "Unsupported architecture '{}': expected x86_64, aarch64, native, amd64, or arm64",
            trimmed
        )),
    }
}

/// Parse a `<name>-<version>.xcs` filename, tolerating prerelease suffixes
/// in the version (`1.0-beta`, `2.0.0-rc.1`). Returns `(name, version)`.
pub fn parse_xcs_name(fname: &str) -> Option<(String, String)> {
    let stem = fname.strip_suffix(".xcs")?;
    let bytes = stem.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let rest = &stem[i + 1..];
            // The version must start with a dotted numeric core; whatever
            // trails it is only acceptable as an empty string, a '-'-led
            // prerelease suffix (which may itself contain dots), or a dot-free
            // appended tag such as "rc1". This keeps garbage like
            // "3.2.1.tar.gz" from parsing as a version.
            let base_end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(rest.len());
            let base = rest[..base_end].trim_end_matches('.');
            if base.is_empty() {
                continue;
            }
            let tail = &rest[base_end..];
            let plausible = tail.is_empty()
                || (tail.starts_with('-')
                    && tail[1..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+')))
                || (!tail.starts_with('.')
                    && !tail.contains('.')
                    && tail
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+')));
            if plausible {
                let name = &stem[..i];
                if name.is_empty() {
                    return None;
                }
                return Some((name.to_string(), rest.to_string()));
            }
        }
    }
    None
}

/// Natural-aware sort key for version strings: runs of digits compare
/// numerically (`1.10` sorts after `1.9`), other runs compare as lowercase
/// text. Deterministic total order.
fn version_sort_key(v: &str) -> Vec<(bool, u64, String)> {
    fn chunk(s: &str, is_num: bool) -> (bool, u64, String) {
        if is_num {
            (true, s.parse().unwrap_or(u64::MAX), String::new())
        } else {
            (false, 0, s.to_lowercase())
        }
    }
    let mut parts: Vec<(bool, u64, String)> = Vec::new();
    let mut cur = String::new();
    let mut cur_is_num: Option<bool> = None;
    for c in v.chars() {
        let is_num = c.is_ascii_digit();
        if cur_is_num != Some(is_num) && !cur.is_empty() {
            parts.push(chunk(&cur, cur_is_num.unwrap_or(false)));
            cur.clear();
        }
        cur_is_num = Some(is_num);
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(chunk(&cur, cur_is_num.unwrap_or(false)));
    }
    parts
}

#[allow(dead_code)] // unused when compiled without the `python` feature
static PLUGIN_MANAGER: OnceLock<Mutex<cps::plugin::PluginManager>> = OnceLock::new();

/// Load plugins declared under `[python].plugins` in ous.toml so pipeline
/// hooks can fire into them. Returns Err when the `python` feature is not
/// compiled in.
pub fn init_plugins(cfg: &cps::PythonConfig) -> Result<()> {
    #[cfg(feature = "python")]
    {
        let mgr = PLUGIN_MANAGER.get_or_init(|| Mutex::new(cps::plugin::PluginManager::new()));
        if let Ok(mut m) = mgr.lock() {
            m.load_all(cfg);
        }
        Ok(())
    }
    #[cfg(not(feature = "python"))]
    {
        let _ = cfg;
        Err(anyhow!(
            "The 'python' feature is required for --plugin run — compile with `cargo build --features python`"
        ))
    }
}

/// Fire a lifecycle hook (`pre-fetch`, `post-build`, …) into loaded plugins.
/// Hook callables use underscores in Python (`pre_fetch`); the name is
/// normalized automatically. Best-effort: never fails the pipeline.
pub fn fire_hook(hook: &str, package: &str) {
    #[cfg(feature = "python")]
    {
        let normalized = hook.replace('-', "_");
        if let Some(mgr) = PLUGIN_MANAGER.get()
            && let Ok(m) = mgr.lock()
        {
            let mut data = HashMap::new();
            data.insert("package".to_string(), package.to_string());
            m.fire(&normalized, &data);
        }
    }
    #[cfg(not(feature = "python"))]
    {
        let _ = (hook, package);
    }
}

/// Quote a value for safe interpolation into a POSIX `sh -c` command string.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ComponentSpec {
    pub name: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub description: String,
}

fn default_priority() -> String {
    "optional".into()
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ServiceSpec {
    pub name: String,
    pub exec: String,
    #[serde(default)]
    pub requires: String,
    #[serde(default = "default_restart")]
    pub restart: String,
    #[serde(default)]
    pub description: String,
}

fn default_restart() -> String {
    "on-failure".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum DependencyEntry {
    Simple(String),
    Versioned { name: String, version: String },
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: String,
    #[serde(rename = "type")]
    pub build_type: String,
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub install: Vec<String>,
    #[serde(default)]
    pub dependencies: Option<Vec<DependencyEntry>>,
    pub links: Option<std::collections::HashMap<String, String>>,
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default)]
    pub components: Option<Vec<ComponentSpec>>,
    #[serde(default)]
    pub services: Option<Vec<ServiceSpec>>,
    #[serde(default)]
    pub binaries: Option<Vec<String>>,
    /// Optional expected SHA-256 of the downloaded source archive. When set,
    /// the archive is verified after download and before extraction.
    #[serde(default)]
    pub sha256: Option<String>,
}

fn default_arch() -> String {
    "native".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub dep_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraries: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Checksum {
    pub kind: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PackageProvenance {
    pub source_type: String,
    pub source_url: String,
    pub source_revision: Option<String>,
    pub built_at: String,
    pub builder: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PackageMetadata {
    pub pkg_name: String,
    pub version: String,
    pub license: String,
    pub source: String,
    #[serde(alias = "arch", rename = "architecture", default)]
    pub arch: String,
    pub checksum: Checksum,
    pub dependencies: Vec<Dependency>,
    pub files: Vec<PathBuf>,
    pub provides: Option<Vec<String>>,
    pub conflicts: Option<Vec<String>>,
    #[serde(default)]
    pub components: Vec<Component>,
    #[serde(default)]
    pub services: Vec<ServiceDecl>,
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default)]
    pub provenance: Option<PackageProvenance>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component {
    pub name: String,
    pub priority: String,
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub dependencies: Vec<ComponentDep>,
    #[serde(default)]
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ComponentDep {
    pub package: String,
    pub component: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ServiceDecl {
    pub name: String,
    pub exec: String,
    #[serde(default)]
    pub requires: String,
    #[serde(default = "default_restart")]
    pub restart: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub working_directory: String,
    #[serde(default)]
    pub socket: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Manifest {
    pub packages: Vec<Package>,
}

pub fn fetch(src: &str, dir: &str, expected_sha256: Option<&str>) -> Result<()> {
    let src_path = if let Some(stripped) = src.strip_prefix("file://") {
        stripped
    } else {
        src
    };

    let src_path_obj = Path::new(src_path);
    if src_path_obj.exists() {
        fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
            fs::create_dir_all(dst)?;
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                let path = entry.path();
                let dest = dst.join(entry.file_name());
                let md = fs::symlink_metadata(&path)?;
                if md.is_dir() {
                    copy_dir_recursive(&path, &dest)?;
                } else if md.file_type().is_symlink() {
                    let target = fs::read_link(&path)?;
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    std::os::unix::fs::symlink(target, &dest)?;
                } else {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&path, &dest)?;
                    // fs::copy preserves the source mode bits; strip
                    // setuid/setgid/sticky so staged trees can't smuggle in
                    // privileged files.
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&dest)?.permissions();
                    let mode = perms.mode();
                    if mode & 0o7000 != 0 {
                        perms.set_mode(mode & 0o777);
                        fs::set_permissions(&dest, perms)?;
                    }
                }
            }
            Ok(())
        }

        let dst = Path::new(dir);
        if src_path_obj.is_dir() {
            UserInterface::info("Copying local source directory...");
            copy_dir_recursive(src_path_obj, dst)
                .context("Failed to copy local source directory")?;
        } else {
            UserInterface::info("Copying local source file...");
            fs::create_dir_all(dst)?;
            let file_name = src_path_obj
                .file_name()
                .ok_or_else(|| anyhow!("Invalid source file name"))?;
            let dest_file = dst.join(file_name);
            fs::copy(src_path_obj, &dest_file).context("Failed to copy local source file")?;
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&dest_file)?.permissions();
            let mode = perms.mode();
            if mode & 0o7000 != 0 {
                perms.set_mode(mode & 0o777);
                fs::set_permissions(&dest_file, perms)?;
            }
        }
        return Ok(());
    }

    if src.ends_with(".git") {
        UserInterface::info("Cloning remote git repository...");
        let status = Command::new("git")
            .args(["clone", "--depth", "1", src, dir])
            .status()?;
        if !status.success() {
            return Err(anyhow!("Git clone failed for source: {}", src));
        }
        return Ok(());
    }

    let archive_name = if src.ends_with(".xz") {
        "temp_archive.tar.xz"
    } else if src.ends_with(".bz2") {
        "temp_archive.tar.bz2"
    } else {
        "temp_archive.tar.gz"
    };

    let archive_path = Path::new(dir).join(archive_name);
    let archive_str = archive_path.to_string_lossy();

    UserInterface::info("Downloading source archive via curl...");
    let curl_status = Command::new("curl")
        .args(["-fSL", "-o", &archive_str, src])
        .status()?;

    if !curl_status.success() {
        return Err(anyhow!("Curl failed to download: {}", src));
    }

    if let Some(expected) = expected_sha256 {
        let actual = hash_file(&archive_path)?
            .into_iter()
            .find(|c| c.kind == "sha256")
            .map(|c| c.value)
            .ok_or_else(|| anyhow!("SHA-256 unavailable for downloaded archive"))?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&archive_path);
            return Err(anyhow!(
                "SHA-256 mismatch for downloaded source '{}': expected {}, got {}",
                src,
                expected,
                actual
            ));
        }
        UserInterface::info("Downloaded archive SHA-256 verified OK");
    }

    let tar_flag = if src.ends_with(".xz") {
        "-xJf"
    } else if src.ends_with(".bz2") {
        "-xjf"
    } else {
        "-xzf"
    };

    UserInterface::info("Extracting source archive...");
    let tar_out = Command::new("tar")
        .args([
            tar_flag,
            &archive_str,
            "-C",
            dir,
            "--strip-components=1",
            "--no-same-owner",
            "--no-same-permissions",
        ])
        .output()?;
    let _ = std::fs::remove_file(&archive_path);

    if !tar_out.status.success() {
        return Err(anyhow!(
            "Tar failed to decompress archive: {}",
            String::from_utf8_lossy(&tar_out.stderr).trim()
        ));
    }

    Ok(())
}

pub fn build(pkg: &Package, dir: &str) -> Result<String> {
    if pkg.build.is_empty() {
        if std::env::var("OUS_NO_AUTO").is_ok() {
            return Ok(String::new());
        }

        if pkg.build_type == "rust" {
            let target = env::var("OUS_TARGET")
                .unwrap_or_else(|_| crate::config::schema::default_target_arch());
            let cpu = if target.contains("aarch64") {
                "armv8-a"
            } else {
                "x86-64-v3"
            };
            UserInterface::info(&format!("Running automatic cargo build for {target}..."));
            let flags = format!(
                "-C linker=clang -C target-cpu={cpu} -C opt-level=3 -C lto=fat -C codegen-units=1 -C target-feature=+crt-static -C link-arg=-target -C link-arg={target} -C link-arg=-march={cpu} -C link-arg=-O3 -C link-arg=-flto=full -C link-arg=--sysroot=/system"
            );
            // Run cargo directly (no shell pipeline) so its real exit status is
            // observed; capture stdout+stderr ourselves into capture.log.
            let out = Command::new("cargo")
                .env("RUSTFLAGS", &flags)
                .args(["build", "--release", "--target", &target])
                .current_dir(dir)
                .output()
                .context("Failed to spawn cargo for automatic Rust build")?;
            let mut log_content = String::from_utf8_lossy(&out.stdout).to_string();
            log_content.push_str(&String::from_utf8_lossy(&out.stderr));
            let log_path = Path::new(dir).join("capture.log");
            fs::write(&log_path, &log_content)?;
            if UserInterface::debug_enabled() {
                UserInterface::info(&format!("cargo output:\n{}", log_content.trim_end()));
            }
            if !out.status.success() {
                return Err(anyhow!(
                    "Rust auto-build failed with {}: {}",
                    out.status,
                    log_content.trim_end()
                ));
            }
            return Ok(log_content);
        } else {
            return Ok(String::new());
        }
    }

    let mut full_log = String::new();

    // Per-command resume markers (from-bit completion): a marker is written
    // only after its build command succeeded, so a failed build re-run starts
    // from the first command that did NOT complete instead of repeating every
    // step from the top. Markers live inside the source dir and are keyed by
    // the command's own SHA-256, so editing the manifest invalidates them,
    // and both --force and --clean bypass them entirely.
    let resume = env::var("OUS_FORCE").is_err() && env::var("OUS_CLEAN").is_err();

    for (i, cmd) in pkg.build.iter().enumerate() {
        let trimmed = cmd.trim();
        if trimmed.eq_ignore_ascii_case("none")
            || trimmed.eq_ignore_ascii_case("skip")
            || trimmed.eq_ignore_ascii_case("nothing")
        {
            UserInterface::warning(&format!("Skipping build command {} as requested", i + 1));
            continue;
        }

        let marker = build_command_marker(dir, i, trimmed);
        if resume && marker.is_file() {
            UserInterface::info(&format!(
                "Resuming from last completed bit — skipping build command {}/{} (use --force to rerun)",
                i + 1,
                pkg.build.len()
            ));
            continue;
        }

        UserInterface::info(&format!(
            "Executing build command {}/{}...",
            i + 1,
            pkg.build.len()
        ));
        // Run via sh but without a pipeline: sh reports THIS command's exit
        // status directly and we write the combined capture.log ourselves.
        let out = Command::new("sh")
            .arg("-c")
            .arg(trimmed)
            .current_dir(dir)
            .output()
            .with_context(|| format!("Failed to spawn shell for build command {}", i + 1))?;
        let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
        combined.push_str(&String::from_utf8_lossy(&out.stderr));

        let log_path = Path::new(dir).join("capture.log");
        fs::write(&log_path, &combined)?;
        if UserInterface::debug_enabled() {
            UserInterface::info(&format!(
                "build command {} output:\n{}",
                i + 1,
                combined.trim_end()
            ));
        }

        if !out.status.success() {
            return Err(anyhow!(
                "Build command {} failed with {}: {}",
                i + 1,
                out.status,
                combined.trim_end()
            ));
        }
        // Command succeeded — drop a durable marker so an interrupted batch
        // continues from the last completed command instead of resubmitting
        // finished work. A failure above leaves no marker for this command.
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&marker, b"ok")?;
        let _ = fs::remove_file(&log_path);
        full_log.push_str(&combined);
    }

    Ok(full_log)
}

/// Path of the from-bit completion marker for build command `index` (0-based)
/// inside the source directory.
fn build_command_marker(dir: &str, index: usize, cmd: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(cmd.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .take(16)
        .collect();
    Path::new(dir)
        .join(".ous-build")
        .join(format!("cmd-{}-{}.done", index + 1, hex))
}

pub fn symlink(target: &str, link_path: &str, root_dir: &str) -> Result<()> {
    let safe_link_path = link_path.trim_start_matches('/');
    if safe_link_path.is_empty() || safe_link_path.ends_with('/') {
        return Err(anyhow!("Invalid symlink link path '{}'", link_path));
    }
    let root = Path::new(root_dir);
    let full_link_path = root.join(safe_link_path);

    // Lexical check: no ".." components may remain after stripping the
    // leading '/' — otherwise the link would be created outside the staging
    // root. (A RootDir component merely reflects that `root_dir` is absolute.)
    use std::path::Component;
    if full_link_path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(anyhow!(
            "Refusing symlink link path '{}': it escapes the staging root",
            link_path
        ));
    }

    if let Some(parent) = full_link_path.parent() {
        fs::create_dir_all(parent)?;
        // Canonical check: resolve any intermediate symlinks and make sure the
        // parent really lives inside the staging root.
        let canon_root = fs::canonicalize(root)?;
        let canon_parent = fs::canonicalize(parent)?;
        if !canon_parent.starts_with(&canon_root) {
            return Err(anyhow!(
                "Refusing symlink link path '{}': resolves outside the staging root",
                link_path
            ));
        }
    }
    let _ = fs::remove_file(&full_link_path);
    std::os::unix::fs::symlink(target, &full_link_path)?;
    Ok(())
}

pub fn install(pkg: &Package, src: &str, dest: &str) -> Result<()> {
    if pkg.install.is_empty() {
        if std::env::var("OUS_NO_AUTO").is_ok() {
            if let Some(links) = &pkg.links {
                for l in links {
                    symlink(l.0, l.1, dest)?;
                }
            }
            return Ok(());
        }

        if pkg.build_type == "rust" {
            UserInterface::info("Running automatic installation for Rust binaries...");
            let target_dir = Path::new(src).join("target").join("release");
            if target_dir.exists() {
                fs::create_dir_all(dest)?;
                if let Ok(entries) = fs::read_dir(&target_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file()
                            && let Some(fname) = path.file_name()
                        {
                            let dest_file = Path::new(dest).join(fname);
                            let _ = fs::remove_file(&dest_file);
                            fs::copy(&path, &dest_file)?;
                        }
                    }
                }
            }

            if let Some(links) = &pkg.links {
                for l in links {
                    symlink(l.0, l.1, dest)?;
                }
            }
            return Ok(());
        }

        if let Some(links) = &pkg.links {
            for l in links {
                symlink(l.0, l.1, dest)?;
            }
        }
        return Ok(());
    }

    for (i, cmd) in pkg.install.iter().enumerate() {
        let trimmed = cmd.trim();
        if trimmed.eq_ignore_ascii_case("none")
            || trimmed.eq_ignore_ascii_case("skip")
            || trimmed.eq_ignore_ascii_case("nothing")
        {
            UserInterface::warning(&format!("Skipping install command {} as requested", i + 1));
            continue;
        }

        UserInterface::info(&format!(
            "Executing install command {}/{}...",
            i + 1,
            pkg.install.len()
        ));
        let status = Command::new("sh")
            .env("CUDANE_DEST", dest)
            .args(["-c", trimmed])
            .current_dir(src)
            .status()?;

        if !status.success() {
            return Err(anyhow!("Install command {} failed", i + 1));
        }
    }

    if let Some(links) = &pkg.links {
        for l in links {
            symlink(l.0, l.1, dest)?;
        }
    }
    Ok(())
}

pub fn hash(dir: &str) -> Result<Vec<Checksum>> {
    let output = Command::new("tar")
        .args(["-cf", "-", "-C", dir, "."])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "tar failed while hashing directory '{}': {}",
            dir,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let sha256 = Sha256::digest(&output.stdout);
    let sha1 = Sha1::digest(&output.stdout);
    let md5 = Md5::digest(&output.stdout);

    Ok(vec![
        Checksum {
            kind: "sha256".to_string(),
            value: sha256.iter().map(|b| format!("{:02x}", b)).collect(),
        },
        Checksum {
            kind: "sha1".to_string(),
            value: sha1.iter().map(|b| format!("{:02x}", b)).collect(),
        },
        Checksum {
            kind: "md5".to_string(),
            value: md5.iter().map(|b| format!("{:02x}", b)).collect(),
        },
    ])
}

/// Hash a single file's raw bytes (used for inspecting packaged archives).
pub fn hash_file(path: &Path) -> Result<Vec<Checksum>> {
    let mut file = fs::File::open(path)?;

    let mut sha256 = Sha256::new();
    let mut sha1 = Sha1::new();
    let mut md5 = Md5::new();

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha256.update(&buf[..n]);
        sha1.update(&buf[..n]);
        md5.update(&buf[..n]);
    }

    let to_hex = |digest: &[u8]| digest.iter().map(|b| format!("{:02x}", b)).collect();
    Ok(vec![
        Checksum {
            kind: "sha256".to_string(),
            value: to_hex(&sha256.finalize()),
        },
        Checksum {
            kind: "sha1".to_string(),
            value: to_hex(&sha1.finalize()),
        },
        Checksum {
            kind: "md5".to_string(),
            value: to_hex(&md5.finalize()),
        },
    ])
}

fn files(dir: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut paths = vec![Path::new(dir).to_path_buf()];
    while let Some(current) = paths.pop() {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    paths.push(path);
                } else if let Ok(rel) = path.strip_prefix(dir) {
                    files.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

fn provides(dir: &str) -> Result<Vec<String>> {
    let mut provides = Vec::new();
    let mut paths = vec![Path::new(dir).to_path_buf()];
    while let Some(current) = paths.pop() {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    paths.push(path);
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && (name.ends_with(".so") || name.contains(".so."))
                {
                    provides.push(name.to_string());
                }
            }
        }
    }
    provides.sort();
    provides.dedup();
    Ok(provides)
}

fn license(src_dir: &str) -> String {
    let license_files = [
        "LICENSE",
        "COPYING",
        "LICENSE.MD",
        "COPYING.MD",
        "MIT-LICENSE",
        "UNLICENSE",
    ];
    let license_regex = Regex::new(
        r"(?i)\b(gnu\s+general\s+public\s+license|gpl|lgpl|agpl|apache|mit|bsd|mpl|mozilla\s+public\s+license|unlicense|isc)\b\s*(v(?:ersion)?\s*\d+(?:\.\d+)?|\d+[-—]clause|\d+(?:\.\d+)?\b)?"
    ).expect("valid license regex");

    if let Ok(entries) = fs::read_dir(src_dir) {
        // Sort entries so the scan is deterministic regardless of
        // filesystem readdir order.
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_uppercase())
                .unwrap_or_default();

            if license_files.iter().any(|&f| name.contains(f))
                && let Ok(content) = fs::read_to_string(&path)
            {
                for cap in license_regex.captures_iter(&content) {
                    let license_name = cap.get(1).map_or("", |m| m.as_str().trim());
                    let mut formatted_name = if license_name.len() <= 4 {
                        license_name.to_uppercase()
                    } else {
                        license_name
                            .split_whitespace()
                            .map(|w| {
                                let mut chars = w.chars();
                                match chars.next() {
                                    Some(first) => {
                                        format!("{}{}", first.to_uppercase(), chars.as_str())
                                    }
                                    None => String::new(),
                                }
                            })
                            .collect::<Vec<String>>()
                            .join(" ")
                    };

                    if let Some(version) = cap.get(2) {
                        let ver_str = version.as_str().trim();
                        if ver_str.to_lowercase().starts_with("v") {
                            let clean_ver = ver_str
                                .trim_start_matches(|c: char| c.is_alphabetic())
                                .trim();
                            formatted_name = format!("{} v{}", formatted_name, clean_ver);
                        } else {
                            formatted_name = format!("{} {}", formatted_name, ver_str);
                        }
                    }

                    if !formatted_name.is_empty() {
                        return formatted_name;
                    }
                }

                if let Some(first_line) = content.lines().find(|l| !l.trim().is_empty()) {
                    let cleaned = first_line
                        .trim()
                        .trim_matches(|c| c == '*' || c == '#' || c == '/')
                        .trim();
                    if !cleaned.is_empty() && cleaned.len() < 60 {
                        return cleaned.to_string();
                    }
                }
            }
        }
    }
    "Unknown".into()
}

pub fn scan(
    dest_dir: &str,
    src_dir: &str,
    log_content: &str,
    current_pkg: &Package,
    repo_root: &Path,
) -> Result<Vec<Dependency>> {
    let mut deps_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut pkg_libs: HashMap<String, Vec<String>> = HashMap::new();

    // Manifest-declared dependencies are authoritative — seed them first so
    // auto-discovery only adds extra types to the same package entry.
    if let Some(declared) = &current_pkg.dependencies {
        for dep in declared {
            let name = match dep {
                DependencyEntry::Simple(n) => n.clone(),
                DependencyEntry::Versioned { name, .. } => name.clone(),
            };
            if name.eq_ignore_ascii_case(&current_pkg.name) {
                continue;
            }
            deps_map
                .entry(name)
                .or_default()
                .insert("Manifest".to_string());
        }
    }

    for (name, dep_type) in crate::deps::scan_source_deps(src_dir) {
        if name.eq_ignore_ascii_case(&current_pkg.name) {
            continue;
        }
        deps_map.entry(name).or_default().insert(dep_type);
    }

    for (name, dep_type) in cdd(log_content) {
        if name.eq_ignore_ascii_case(&current_pkg.name) {
            continue;
        }
        deps_map.entry(name).or_default().insert(dep_type);
    }

    let library_names = crate::deps::libdeps(dest_dir)?;
    let library_packages = mltp(repo_root, &current_pkg.arch)?;
    let strict = env::var("OUS_STRICT").is_ok();
    let mut unresolved: Vec<String> = Vec::new();

    for lib in library_names {
        let normalized = normalize(&lib);
        let mut resolved = false;

        for candidate in &normalized {
            if let Some(package_names) = library_packages.get(candidate) {
                for package_name in package_names {
                    if package_name.eq_ignore_ascii_case(&current_pkg.name) {
                        continue;
                    }
                    deps_map
                        .entry(package_name.clone())
                        .or_default()
                        .insert(format!("Library ({})", lib));
                    pkg_libs
                        .entry(package_name.clone())
                        .or_default()
                        .push(lib.clone());
                    resolved = true;
                }
            }
        }

        if !resolved {
            unresolved.push(lib.clone());
            deps_map
                .entry(lib)
                .or_default()
                .insert("Library".to_string());
        }
    }

    if strict && !unresolved.is_empty() {
        return Err(anyhow!(
            "Strict mode: libraries could not be resolved to packages: {}",
            unresolved.join(", ")
        ));
    }

    let index_graph = loadex(repo_root, &current_pkg.arch)?;
    let mut visited: HashSet<String> = HashSet::new();
    for package_name in deps_map.keys().cloned().collect::<Vec<_>>() {
        transitive(
            &package_name,
            &index_graph,
            &mut visited,
            &current_pkg.name,
            &mut deps_map,
        );
    }

    let mut final_deps = Vec::new();
    for (name, types) in deps_map {
        let mut types_vec: Vec<String> = types.into_iter().collect();
        types_vec.sort();

        let libraries = pkg_libs.get(&name).and_then(|libs| {
            if libs.len() >= 2 {
                let mut sorted = libs.clone();
                sorted.sort();
                sorted.dedup();
                Some(sorted)
            } else {
                None
            }
        });

        final_deps.push(Dependency {
            name,
            dep_type: types_vec.join(" & "),
            libraries,
        });
    }

    final_deps.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(final_deps)
}

fn cdd(log_content: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();

    let re = Regex::new(r"(?i)pkg-config[^\n]*--libs\s+([^\s]+)").expect("valid regex");
    let dep_colon_re =
        Regex::new(r"(?i)(?:dependency|package)\b[^:\n]*:\s*([^\s]+)").expect("valid regex");
    for line in log_content.lines() {
        let lower = line.to_lowercase();
        let mut extracted_name = String::new();

        if lower.contains("pkg-config") {
            if let Some(caps) = re.captures(line) {
                extracted_name = caps[1].to_string();
            }
        } else if (lower.contains("dependency") || lower.contains("package"))
            && lower.contains("found")
        {
            if let Some(caps) = dep_colon_re.captures(line) {
                extracted_name = caps[1].to_string();
            } else {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(idx) = parts
                    .iter()
                    .position(|&r| r.eq_ignore_ascii_case("dependency"))
                {
                    if idx + 1 < parts.len() {
                        extracted_name = parts[idx + 1].to_string();
                    }
                } else if let Some(idx) = parts
                    .iter()
                    .position(|&r| r.eq_ignore_ascii_case("package"))
                    && idx + 1 < parts.len()
                {
                    extracted_name = parts[idx + 1].to_string();
                }
            }
        } else if lower.starts_with("found ") || lower.starts_with("checking for ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                extracted_name = parts[1].to_string();
            }
        }

        let clean_name = extracted_name
            .trim_matches(|c| {
                c == '\''
                    || c == '"'
                    || c == '`'
                    || c == ':'
                    || c == '.'
                    || c == ','
                    || c == ';'
                    || c == '('
                    || c == ')'
                    || c == '['
                    || c == ']'
                    || c == '{'
                    || c == '}'
                    || c == '/'
            })
            .trim()
            .to_string();

        let ignore_list = [
            "threads",
            "for",
            "pkg-config",
            "cmake",
            "ninja",
            "yes",
            "no",
            "found",
            "not",
            "module",
            "function",
            "program",
            "library",
        ];
        if !clean_name.is_empty() && !ignore_list.contains(&clean_name.to_lowercase().as_str()) {
            results.push((clean_name, "Build".to_string()));
        }
    }

    results
}

fn normalize(lib: &str) -> Vec<String> {
    let mut normalized = Vec::new();
    normalized.push(lib.to_string());

    if let Some(stripped) = lib.strip_prefix("lib") {
        normalized.push(stripped.to_string());

        if let Some(pos) = stripped.find('.') {
            normalized.push(format!("lib{}", &stripped[..pos]));
        }
    }

    if let Some(pos) = lib.find(".so") {
        normalized.push(lib[..pos + 3].to_string());
    }

    normalized.sort();
    normalized.dedup();
    normalized
}

fn mltp(repo_root: &Path, arch: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut library_packages: HashMap<String, Vec<String>> = HashMap::new();
    let ous_root = repo_root.join(".ous");

    if !ous_root.exists() {
        return Ok(library_packages);
    }

    if let Ok(entries) = fs::read_dir(&ous_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let package_name = entry.file_name().to_string_lossy().to_string();
            let mut pkg_paths = Vec::new();
            if path.join("pkg").exists() {
                pkg_paths.push(path.join("pkg"));
            }
            if !arch.is_empty() && arch != "native" && path.join(arch).join("pkg").exists() {
                pkg_paths.push(path.join(arch).join("pkg"));
            }

            while let Some(current_dir) = pkg_paths.pop() {
                if let Ok(entries) = fs::read_dir(&current_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            pkg_paths.push(path);
                            continue;
                        }

                        if let Some(name) = path.file_name().and_then(|n| n.to_str())
                            && (name.contains(".so")
                                || name.ends_with(".dll")
                                || name.ends_with(".dylib")
                                || name.ends_with(".a"))
                        {
                            for variant in normalize(name) {
                                let pkg_list = library_packages.entry(variant).or_default();
                                if !pkg_list.contains(&package_name) {
                                    pkg_list.push(package_name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(library_packages)
}

fn loadex(repo_root: &Path, arch: &str) -> Result<HashMap<String, HashSet<String>>> {
    let arch_name = if arch.is_empty() || arch == "native" {
        "native".to_string()
    } else {
        arch.to_string()
    };
    let index_path = repo_root.join(format!("index.{}.json", arch_name));
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();

    if !index_path.exists() {
        return Ok(graph);
    }

    let index_content = fs::read_to_string(&index_path)?;
    let packages: Vec<PackageMetadata> = serde_json::from_str(&index_content)
        .map_err(|e| anyhow!("Failed to parse index {}: {}", index_path.display(), e))?;

    for pkg in packages {
        let dep_names = pkg
            .dependencies
            .iter()
            .map(|d| d.name.clone())
            .collect::<HashSet<_>>();
        graph.entry(pkg.pkg_name).or_default().extend(dep_names);
    }

    Ok(graph)
}

fn transitive(
    package_name: &str,
    index_graph: &HashMap<String, HashSet<String>>,
    visited: &mut HashSet<String>,
    current_pkg_name: &str,
    deps_map: &mut HashMap<String, HashSet<String>>,
) {
    if visited.contains(package_name) || package_name.eq_ignore_ascii_case(current_pkg_name) {
        return;
    }

    visited.insert(package_name.to_string());

    if let Some(children) = index_graph.get(package_name) {
        for child in children {
            if child.eq_ignore_ascii_case(current_pkg_name) {
                continue;
            }
            deps_map
                .entry(child.clone())
                .or_default()
                .insert("Transitive".to_string());
            transitive(child, index_graph, visited, current_pkg_name, deps_map);
        }
    }
}

/// Format a `SystemTime` as RFC 3339 UTC without external crates.
fn iso8601_utc(st: std::time::SystemTime) -> String {
    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
    let dur = st.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

pub fn detect_source_type(source: &str) -> String {
    if source.starts_with("git@") || source.starts_with("git://") || source.ends_with(".git") {
        "git".to_string()
    } else if source.starts_with("https://") || source.starts_with("http://") {
        "http".to_string()
    } else if source.starts_with("file://") {
        "file".to_string()
    } else if source.contains("://") {
        "url".to_string()
    } else {
        "dir".to_string()
    }
}

/// Build a provenance record for a package at the given source directory.
/// `source` is the original manifest source value. `src_dir` is the
/// materialized source directory on disk (may be empty for manual builds).
pub fn build_provenance(source: &str, src_dir: &str) -> PackageProvenance {
    let source_type = detect_source_type(source);
    let source_revision = if source_type == "git" && !src_dir.is_empty() {
        Command::new("git")
            .args(["-C", src_dir, "rev-parse", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            })
    } else {
        None
    };
    PackageProvenance {
        source_type,
        source_url: source.to_string(),
        source_revision,
        built_at: iso8601_utc(std::time::SystemTime::now()),
        builder: format!("ous-{}", env!("CARGO_PKG_VERSION")),
    }
}

pub fn mtd(
    pkg: &Package,
    dest: &str,
    sum: &[Checksum],
    src_dir: &str,
    log_content: &str,
    repo_root: &Path,
    provenance: Option<PackageProvenance>,
) -> Result<PackageMetadata> {
    let dependencies = scan(dest, src_dir, log_content, pkg, repo_root)?;
    let pkg_files = files(dest)?;
    let provides = provides(dest)?;

    let target_type = match env::var("OUS_HASH_TYPE") {
        Ok(raw) => {
            let lowered = raw.to_lowercase();
            if matches!(lowered.as_str(), "sha256" | "sha1" | "md5") {
                lowered
            } else {
                return Err(anyhow!(
                    "Invalid hash type '{}': supported algorithms are sha256, sha1, md5",
                    raw
                ));
            }
        }
        Err(_) => "sha256".to_string(),
    };

    let selected = if let Some(c) = sum.iter().find(|c| c.kind == target_type) {
        c.clone()
    } else if let Some(first) = sum.first() {
        first.clone()
    } else {
        return Err(anyhow!("No checksum values available for {}", pkg.name));
    };

    let components = assign_components(&pkg_files, pkg);

    let mut services: Vec<ServiceDecl> = Vec::new();
    if let Some(ref specs) = pkg.services {
        for s in specs {
            services.push(ServiceDecl {
                name: s.name.clone(),
                exec: s.exec.clone(),
                requires: s.requires.clone(),
                restart: s.restart.clone(),
                description: s.description.clone(),
                environment: std::collections::HashMap::new(),
                working_directory: String::new(),
                socket: None,
            });
        }
    }

    let binaries: Vec<String> = pkg.binaries.clone().unwrap_or_default();

    Ok(PackageMetadata {
        pkg_name: pkg.name.clone(),
        version: pkg.version.clone(),
        license: license(src_dir),
        source: pkg.source.clone(),
        arch: pkg.arch.clone(),
        checksum: selected,
        dependencies,
        files: pkg_files.into_iter().map(PathBuf::from).collect(),
        provides: Some(provides),
        conflicts: None::<Vec<String>>,
        components,
        services,
        binaries,
        provenance,
    })
}

pub fn assign_components(pkg_files: &[String], pkg: &Package) -> Vec<Component> {
    let specs = match &pkg.components {
        Some(specs) if !specs.is_empty() => specs.clone(),
        _ => {
            return vec![Component {
                name: "core".to_string(),
                priority: "required".to_string(),
                files: pkg_files.iter().map(PathBuf::from).collect(),
                dependencies: Vec::new(),
                description: format!("Core files for {}", pkg.name),
            }];
        }
    };

    let mut assigned: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut unassigned_files: HashSet<String> = pkg_files.iter().cloned().collect();

    for spec in &specs {
        let mut matched = Vec::new();
        for raw_pattern in &spec.files {
            // Trailing slashes would never match the stored relative paths.
            let pattern = raw_pattern.trim_end_matches('/');
            if pattern.is_empty() {
                UserInterface::error(&format!(
                    "Component '{}': empty file pattern is not allowed",
                    spec.name
                ));
                continue;
            }
            for file in pkg_files {
                // First matching spec wins: files already claimed by an
                // earlier component are skipped here.
                if !unassigned_files.contains(file) {
                    continue;
                }
                let boundary_match = file.starts_with(pattern)
                    && file.is_char_boundary(pattern.len())
                    && file[pattern.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| c == '/');
                if file == pattern || boundary_match {
                    matched.push(PathBuf::from(file));
                    unassigned_files.remove(file);
                }
            }
        }
        matched.sort();
        matched.dedup();
        assigned.insert(spec.name.clone(), matched);
    }

    if !unassigned_files.is_empty() {
        let core_files: Vec<PathBuf> = unassigned_files.into_iter().map(PathBuf::from).collect();
        let entry = assigned.entry("core".to_string()).or_default();
        entry.extend(core_files);
        entry.sort();
        entry.dedup();

        if !specs.iter().any(|s| s.name == "core") {
            UserInterface::warning(&format!(
                "{} files not matched by any component; adding to 'core'",
                entry.len()
            ));
        }
    }

    let mut components: Vec<Component> = specs
        .iter()
        .map(|spec| {
            let files = assigned.get(&spec.name).cloned().unwrap_or_default();
            Component {
                name: spec.name.clone(),
                priority: spec.priority.clone(),
                files,
                dependencies: Vec::new(),
                description: spec.description.clone(),
            }
        })
        .collect();

    if let Some(core_files) = assigned.get("core")
        && !core_files.is_empty()
        && !specs.iter().any(|s| s.name == "core")
    {
        components.push(Component {
            name: "core".to_string(),
            priority: "required".to_string(),
            files: core_files.clone(),
            dependencies: Vec::new(),
            description: format!("Core files for {}", pkg.name),
        });
    }

    let has_required = components.iter().any(|c| c.priority == "required");
    if !has_required
        && !components.is_empty()
        && let Some(first) = components.first_mut()
    {
        first.priority = "required".to_string();
    }

    components.sort_by(|a, b| a.name.cmp(&b.name));
    components
}

pub fn meta(
    pkg: &Package,
    dest: &str,
    sum: &[Checksum],
    src_dir: &str,
    log_content: &str,
) -> Result<()> {
    let repo_root = env::current_dir()?;
    let prov = Some(build_provenance(&pkg.source, src_dir));
    let meta = mtd(pkg, dest, sum, src_dir, log_content, &repo_root, prov)?;
    write(&meta, dest)
}

pub fn write(meta: &PackageMetadata, dest: &str) -> Result<()> {
    fs::create_dir_all(dest)?;
    let path = format!("{}/metadata.json", dest);
    let mut out = meta.clone();
    out.arch = canonical_arch(&out.arch)?;
    let json = serde_json::to_string_pretty(&out)?;
    atomic_write(Path::new(&path), json.as_bytes())?;
    Ok(())
}

pub fn index(index_root: &str, meta: &PackageMetadata) -> Result<()> {
    let arch = canonical_arch(&meta.arch)?;
    let index_path = Path::new(index_root).join(format!("index.{}.json", arch));
    fs::create_dir_all(index_root)?;

    let mut entries: Vec<PackageMetadata> = if index_path.exists() {
        let existing = fs::read_to_string(&index_path)?;
        serde_json::from_str(&existing)
            .map_err(|e| anyhow!("Failed to parse index {}: {}", index_path.display(), e))?
    } else {
        Vec::new()
    };

    if let Some(existing_meta) = entries
        .iter_mut()
        .find(|entry| entry.pkg_name == meta.pkg_name && entry.version == meta.version)
    {
        if *existing_meta == *meta {
            return Ok(());
        }
        *existing_meta = meta.clone();
    } else {
        entries.push(meta.clone());
    }

    // Sort by name, then by version using a natural-aware key so numeric
    // runs compare numerically (1.10 sorts after 1.9).
    entries.sort_by(|a, b| {
        a.pkg_name
            .cmp(&b.pkg_name)
            .then_with(|| version_sort_key(&a.version).cmp(&version_sort_key(&b.version)))
    });
    let json = serde_json::to_string_pretty(&entries)?;
    atomic_write(&index_path, json.as_bytes())?;

    Ok(())
}

pub fn archive(dest: &str, out: &str) -> Result<()> {
    let level = zstd_level_from_env();
    let out_path = Path::new(out);
    let dir = out_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = out_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "package.xcs".to_string());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp_path = dir.join(format!(
        ".{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        nanos
    ));

    // tar | zstd pipeline writing into a temporary sibling; only a fully
    // successful run is renamed over the final output path.
    let mut tar_child = Command::new("tar")
        .args(["-c", "-C", dest, "."])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let tar_stdin = tar_child.stdout.take();

    let zstd_child = match tar_stdin {
        Some(stdin) => fs::File::create(&tmp_path).and_then(|file| {
            Command::new("zstd")
                .arg(format!("-{}", level))
                .stdin(std::process::Stdio::from(stdin))
                .stdout(std::process::Stdio::from(file))
                .spawn()
        }),
        None => Err(std::io::Error::other("tar stdout unavailable")),
    };

    let mut zstd_child = match zstd_child {
        Ok(child) => child,
        Err(e) => {
            // Never leak/zombify the tar child when the downstream half of
            // the pipeline fails to start.
            let _ = tar_child.kill();
            let _ = tar_child.wait();
            return Err(anyhow!(e).context("Failed to start zstd for archive compression"));
        }
    };

    let tar_status = tar_child.wait()?;
    let zstd_result = zstd_child.wait();

    match (tar_status.success(), zstd_result) {
        (true, Ok(zstd_status)) if zstd_status.success() => {
            fs::rename(&tmp_path, out_path)?;
            Ok(())
        }
        (_, zstd_status) => {
            let detail = match zstd_status {
                Ok(s) => format!("zstd exited with {}", s),
                Err(e) => format!("zstd wait failed: {}", e),
            };
            let _ = fs::remove_file(&tmp_path);
            Err(anyhow!(
                "Archive compression failed (tar: {}, {})",
                if tar_status.success() { "ok" } else { "failed" },
                detail
            ))
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct BuildProgress {
    completed_steps: HashSet<String>,
}

fn save_state(path: &Path, state: &BuildProgress) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    atomic_write(path, json.as_bytes())?;
    Ok(())
}

fn validate_path_component(value: &str, label: &str) -> Result<()> {
    let safe = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if value.is_empty()
        || value.contains('/')
        || value.contains("..")
        || value.starts_with('.')
        || !safe
    {
        return Err(anyhow!(
            "Invalid {} '{}': only letters, digits, '-', '_' and '.' are allowed; '/', '..' and a leading '.' are forbidden",
            label,
            value
        ));
    }
    Ok(())
}

pub fn process(pkg: &Package, out_dir: &str) -> Result<String> {
    let canonical = canonical_arch(&pkg.arch)?;
    validate_path_component(&pkg.name, "package name")?;
    validate_path_component(&canonical, "package arch")?;
    validate_version_component(&pkg.version, "package version")?;
    let mut pkg = pkg.clone();
    pkg.arch = canonical;
    UserInterface::info(&format!(
        "Processing package: {} v{}",
        pkg.name, pkg.version
    ));
    let current_dir = env::current_dir()?;

    let absolute_out_dir = current_dir.join(out_dir);
    fs::create_dir_all(&absolute_out_dir)?;

    let final_path = format!(
        "{}/{}-{}.xcs",
        absolute_out_dir.display(),
        pkg.name,
        pkg.version
    );
    // Short-circuit only when neither --force nor --clean was requested:
    // --force must rebuild the archive, --clean starts from a fresh workspace.
    if Path::new(&final_path).exists()
        && env::var("OUS_FORCE").is_err()
        && env::var("OUS_CLEAN").is_err()
    {
        UserInterface::success(&format!(
            "Package archive already exists at: {}",
            final_path
        ));
        upload_step(&pkg, &final_path, &current_dir)?;
        return Ok(final_path);
    }

    let arch_dir = if pkg.arch.is_empty() || pkg.arch == "native" {
        pkg.name.clone()
    } else {
        format!("{}/{}", pkg.name, pkg.arch)
    };
    // OUS_PROJECT_WORKSPACE relocates the build workspace root (chroot-style
    // base for workspaces); defaults to the current directory.
    let workspace_root = match env::var("OUS_PROJECT_WORKSPACE") {
        Ok(dir) if !dir.trim().is_empty() => {
            let p = PathBuf::from(dir);
            if p.is_absolute() {
                p
            } else {
                current_dir.join(p)
            }
        }
        _ => current_dir.clone(),
    };
    let work_dir = workspace_root.join(format!(".ous/{}", arch_dir));
    let src_dir = work_dir.join("src");
    let pkg_root = work_dir.join("pkg");
    let state_path = work_dir.join(".state.json");
    let build_log_path = work_dir.join("ous.log");
    let sum_path = work_dir.join("checksums.json");

    let mut state = BuildProgress::default();
    let clean = env::var("OUS_CLEAN").is_ok();
    let force = env::var("OUS_FORCE").is_ok();

    if clean || !state_path.exists() {
        let _ = fs::remove_dir_all(&work_dir);
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&pkg_root)?;
    } else {
        if state_path.exists() {
            match serde_json::from_str(&fs::read_to_string(&state_path)?) {
                Ok(s) => state = s,
                Err(e) => UserInterface::warning(&format!(
                    "Could not parse {} ({}); resuming degraded to a cold build",
                    state_path.display(),
                    e
                )),
            }
        }
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&pkg_root)?;
    }

    // --force must produce a fresh archive: drop the tail steps from the
    // resume markers so hash/metadata/archive re-run even when previously
    // completed.
    if force {
        for step in ["archive", "hash", "metadata"] {
            state.completed_steps.remove(step);
        }
    }

    let src_str = src_dir.to_string_lossy().to_string();
    let root_str = pkg_root.to_string_lossy().to_string();

    if !(state.completed_steps.contains("fetch") && dir_non_empty(&src_dir)) {
        if state.completed_steps.contains("fetch") {
            UserInterface::warning(
                "Resume marker says fetch is done but src/ is empty — refetching",
            );
        }
        UserInterface::info("Fetching package source...");
        fire_hook("pre-fetch", &pkg.name);
        if let Err(e) = fetch(&pkg.source, &src_str, pkg.sha256.as_deref()) {
            UserInterface::error(&format!("Fetch step failed: {}", e));
            return Err(anyhow!(e).context("Fetch step failed"));
        }
        fire_hook("post-fetch", &pkg.name);
        state.completed_steps.insert("fetch".to_string());
        save_state(&state_path, &state)?;
    }

    let build_log = if state.completed_steps.contains("build") && build_log_path.is_file() {
        match fs::read_to_string(&build_log_path) {
            Ok(log) => log,
            Err(e) => {
                UserInterface::warning(&format!(
                    "Could not read persisted build log ({}); rerunning build",
                    e
                ));
                run_build_step(&pkg, &src_str, &build_log_path, &mut state, &state_path)?
            }
        }
    } else {
        if state.completed_steps.contains("build") {
            UserInterface::warning(
                "Resume marker says build is done but no build log found — rebuilding",
            );
        }
        run_build_step(&pkg, &src_str, &build_log_path, &mut state, &state_path)?
    };

    if !(state.completed_steps.contains("install") && dir_non_empty(&pkg_root)) {
        if state.completed_steps.contains("install") {
            UserInterface::warning(
                "Resume marker says install is done but pkg/ is empty — reinstalling",
            );
        }
        UserInterface::info("Installing built files to root target...");
        fire_hook("pre-install", &pkg.name);
        if let Err(e) = install(&pkg, &src_str, &root_str) {
            UserInterface::error(&format!("Install step failed: {}", e));
            return Err(anyhow!(e).context("Install step failed"));
        }
        fire_hook("post-install", &pkg.name);
        state.completed_steps.insert("install".to_string());
        save_state(&state_path, &state)?;
    }

    let sum: Vec<Checksum> = if state.completed_steps.contains("hash") && sum_path.exists() {
        match serde_json::from_str(&fs::read_to_string(&sum_path)?) {
            Ok(s) => s,
            Err(e) => {
                UserInterface::warning(&format!(
                    "Could not parse persisted checksums ({}); recomputing",
                    e
                ));
                compute_hash_step(&root_str, &sum_path, &mut state, &state_path, &pkg.name)?
            }
        }
    } else {
        if state.completed_steps.contains("hash") {
            UserInterface::warning(
                "Resume marker says hash is done but checksums.json is missing — recomputing",
            );
        }
        compute_hash_step(&root_str, &sum_path, &mut state, &state_path, &pkg.name)?
    };

    if !state.completed_steps.contains("metadata") {
        UserInterface::info("Compiling dependency graph and manifest metadata...");
        fire_hook("pre-metadata", &pkg.name);
        let prov = Some(build_provenance(&pkg.source, &src_str));
        let metadata = match mtd(
            &pkg,
            &root_str,
            &sum,
            &src_str,
            &build_log,
            current_dir.as_path(),
            prov,
        ) {
            Ok(meta) => meta,
            Err(e) => {
                UserInterface::error(&format!("Metadata generation failed: {}", e));
                return Err(anyhow!(e).context("Metadata generation failed"));
            }
        };
        if let Err(e) = write(&metadata, &root_str) {
            UserInterface::error(&format!("Writing metadata json failed: {}", e));
            return Err(anyhow!(e).context("Metadata generation failed"));
        }
        if let Err(e) = index(&current_dir.to_string_lossy(), &metadata) {
            UserInterface::error(&format!("Repository index append failed: {}", e));
            return Err(anyhow!(e).context("Repository index generation failed"));
        }
        fire_hook("post-metadata", &pkg.name);
        state.completed_steps.insert("metadata".to_string());
        save_state(&state_path, &state)?;
    }

    if !state.completed_steps.contains("archive") {
        UserInterface::info("Compressing target root into final .xcs package...");
        fire_hook("pre-archive", &pkg.name);
        if let Err(e) = archive(&root_str, &final_path) {
            UserInterface::error(&format!("Archiving compression failed: {}", e));
            return Err(anyhow!(e).context("Archiving step failed"));
        }
        // Write the integrity sidecar: hex SHA-256 of the FINAL compressed
        // archive. `ous --inspect` verifies the .xcs against this digest.
        match hash_file(Path::new(&final_path)) {
            Ok(sums) => {
                if let Some(sha) = sums.iter().find(|c| c.kind == "sha256") {
                    let sidecar = format!("{}.sha256", final_path);
                    if let Err(e) =
                        atomic_write(Path::new(&sidecar), format!("{}\n", sha.value).as_bytes())
                    {
                        UserInterface::warning(&format!(
                            "Could not write integrity sidecar {}: {}",
                            sidecar, e
                        ));
                    }
                }
            }
            Err(e) => {
                UserInterface::warning(&format!("Could not hash archive for sidecar: {}", e));
            }
        }
        fire_hook("post-archive", &pkg.name);
        state.completed_steps.insert("archive".to_string());
        save_state(&state_path, &state)?;
    }

    fire_hook("done", &pkg.name);
    upload_step(&pkg, &final_path, &current_dir)?;
    Ok(final_path)
}

/// Upload the finished archive (plus `.sha256` sidecar and, when requested,
/// the updated index) to the repository referenced by `OUS_UPLOAD_URL`. A
/// no-op when no upload URL is configured.
fn upload_step(pkg: &Package, final_path: &str, cwd: &Path) -> Result<()> {
    let Ok(base_url) = env::var("OUS_UPLOAD_URL") else {
        return Ok(());
    };
    if base_url.trim().is_empty() {
        return Ok(());
    }
    let arch = canonical_arch(&pkg.arch)?;
    let opts = crate::upload::UploadOptions {
        base_url: base_url.trim().to_string(),
        token: env::var("OUS_UPLOAD_TOKEN").ok(),
        arch: arch.clone(),
        upload_index: env::var("OUS_UPLOAD_INDEX").is_ok(),
    };
    let index_source = cwd.join(crate::upload::index_path(&arch)?);
    let uploaded = crate::upload::upload_package(
        &opts,
        Path::new(final_path),
        &pkg.name,
        &pkg.version,
        Some(&index_source),
    )?;
    UserInterface::success(&format!(
        "Published {} v{} — {} file(s) uploaded",
        pkg.name,
        pkg.version,
        uploaded.len()
    ));
    Ok(())
}

fn run_build_step(
    pkg: &Package,
    src_str: &str,
    build_log_path: &Path,
    state: &mut BuildProgress,
    state_path: &Path,
) -> Result<String> {
    UserInterface::info("Building package modules...");
    fire_hook("pre-build", &pkg.name);
    let log = match build(pkg, src_str) {
        Ok(log) => log,
        Err(e) => {
            UserInterface::error(&format!("Build step failed: {}", e));
            return Err(anyhow!(e).context("Build step failed"));
        }
    };
    atomic_write(build_log_path, log.as_bytes())?;
    fire_hook("post-build", &pkg.name);
    state.completed_steps.insert("build".to_string());
    save_state(state_path, state)?;
    Ok(log)
}

fn compute_hash_step(
    root_str: &str,
    sum_path: &Path,
    state: &mut BuildProgress,
    state_path: &Path,
    pkg_name: &str,
) -> Result<Vec<Checksum>> {
    UserInterface::info("Generating build checksum hash...");
    fire_hook("pre-hash", pkg_name);
    let s = match hash(root_str) {
        Ok(s) => s,
        Err(e) => {
            UserInterface::error(&format!("Hashing step failed: {}", e));
            return Err(anyhow!(e).context("Hashing step failed"));
        }
    };
    atomic_write(sum_path, serde_json::to_string(&s)?.as_bytes())?;
    fire_hook("post-hash", pkg_name);
    state.completed_steps.insert("hash".to_string());
    save_state(state_path, state)?;
    Ok(s)
}

pub fn sort_packages(dir: &str, arch: &str) -> Result<()> {
    let pool_dir = Path::new(dir);
    if !pool_dir.exists() {
        return Err(anyhow!("Directory '{}' not found", dir));
    }
    let force = env::var("OUS_FORCE").is_ok();
    let mut moved = 0;
    let mut skipped = 0;
    for entry in fs::read_dir(pool_dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().into_owned();
        if !fname.ends_with(".xcs") || !entry.path().is_file() {
            continue;
        }
        let Some((pkg_name, _version)) = parse_xcs_name(&fname) else {
            UserInterface::warning(&format!(
                "Skipping '{}': filename does not match <name>-<version>.xcs",
                fname
            ));
            skipped += 1;
            continue;
        };
        let target_dir = pool_dir.join(arch).join(&pkg_name);
        fs::create_dir_all(&target_dir)?;
        let target = target_dir.join(&fname);
        if target.exists() && !force {
            // Destructive overwrite: require --force or an explicit yes.
            let proceed = env::var("OUS_ASSUME_YES").is_ok()
                || UserInterface::prompt_confirmation(&format!(
                    "Overwrite existing {}?",
                    target.display()
                ));
            if !proceed {
                UserInterface::warning(&format!(
                    "Skipped (exists): {} -> {}",
                    fname,
                    target.display()
                ));
                skipped += 1;
                continue;
            }
        }
        fs::rename(entry.path(), &target)?;
        if env::var("OUS_QUIET").is_err() {
            UserInterface::info(&format!("Moved: {} -> {}/{}", fname, arch, pkg_name));
        }
        moved += 1;
    }
    if skipped > 0 {
        UserInterface::warning(&format!(
            "Sorted {} packages into {}/{}/ ({} skipped)",
            moved, dir, arch, skipped
        ));
    } else {
        UserInterface::success(&format!("Sorted {} packages into {}/{}/", moved, dir, arch));
    }
    Ok(())
}

pub fn validate(index_path: &str, packages_dir: &str) -> Result<usize> {
    let mut problems = 0usize;
    let index_file = Path::new(index_path);
    let mut index_entries: Vec<PackageMetadata> = Vec::new();

    if !index_file.exists() {
        UserInterface::warning(&format!(
            "{} not found — skipping index validation",
            index_path
        ));
    } else {
        let content = fs::read_to_string(index_file)?;
        match serde_json::from_str::<Vec<PackageMetadata>>(&content) {
            Ok(data) => {
                index_entries = data;
                UserInterface::success(&format!(
                    "{}: valid flat array ({} packages)",
                    index_path,
                    index_entries.len()
                ));
                let mut seen = HashSet::new();
                for entry in &index_entries {
                    if entry.pkg_name.is_empty() {
                        UserInterface::error("entry: missing pkg_name");
                        problems += 1;
                    }
                    if entry.version.is_empty() {
                        UserInterface::error(&format!("{}: missing version", entry.pkg_name));
                        problems += 1;
                    }
                    if seen.contains(&(entry.pkg_name.clone(), entry.version.clone())) {
                        UserInterface::error(&format!(
                            "{} v{}: duplicate entry",
                            entry.pkg_name, entry.version
                        ));
                        problems += 1;
                    }
                    seen.insert((entry.pkg_name.clone(), entry.version.clone()));
                }
            }
            Err(e) => {
                UserInterface::error(&format!("{} is not valid JSON: {}", index_path, e));
                problems += 1;
            }
        }
    }

    let pkg_dir = Path::new(packages_dir);
    if pkg_dir.exists() && !index_entries.is_empty() {
        let index_names: HashSet<String> =
            index_entries.iter().map(|e| e.pkg_name.clone()).collect();
        let mut built_names = HashSet::new();
        for entry in fs::read_dir(pkg_dir)? {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().into_owned();
            if fname.ends_with(".xcs") {
                match parse_xcs_name(&fname) {
                    Some((name, _version)) => {
                        built_names.insert(name);
                    }
                    None => {
                        UserInterface::warning(&format!(
                            "{}: filename does not match <name>-<version>.xcs",
                            fname
                        ));
                        problems += 1;
                    }
                }
            }
        }
        let orphaned: Vec<&String> = built_names.difference(&index_names).collect();
        for name in &orphaned {
            UserInterface::warning(&format!(
                "{}: built .xcs found but no entry in {}",
                name, index_path
            ));
        }
        let missing: Vec<&String> = index_names.difference(&built_names).collect();
        for name in &missing {
            UserInterface::warning(&format!(
                "{}: in {} but no .xcs in {}",
                name, index_path, packages_dir
            ));
        }
        UserInterface::success(&format!(
            "{} built, {} indexed, {} orphaned, {} missing",
            built_names.len(),
            index_entries.len(),
            orphaned.len(),
            missing.len()
        ));
    }

    if pkg_dir.exists() {
        for entry in fs::read_dir(pkg_dir)? {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().into_owned();
            if !fname.ends_with(".xcs") {
                continue;
            }
            let path = entry.path();
            let mut file = fs::File::open(&path)?;
            let mut magic = [0u8; 4];
            if file.read_exact(&mut magic).is_ok() && magic != [0x28, 0xB5, 0x2F, 0xFD] {
                UserInterface::warning(&format!("{}: not a valid zstd archive (bad magic)", fname));
                problems += 1;
            }
        }
    }

    if problems > 0 {
        UserInterface::error(&format!("Validation: {} issue(s) found", problems));
    } else {
        UserInterface::success("Validation: all checks passed");
    }
    Ok(problems)
}

pub fn checksum_index(index_path: &str, pkg_dir: &str, base_url: &str, arch: &str) -> Result<()> {
    let index_file = Path::new(index_path);
    if !index_file.exists() {
        UserInterface::warning(&format!("{} not found — skipping", index_path));
        return Ok(());
    }
    let content = fs::read_to_string(index_file)?;
    let mut index: Vec<PackageMetadata> = serde_json::from_str(&content)?;

    let mut by_key: HashMap<(String, String), usize> = HashMap::new();
    let mut duplicates = 0usize;
    for (i, entry) in index.iter().enumerate() {
        let key = (entry.pkg_name.clone(), entry.version.clone());
        if by_key.insert(key.clone(), i).is_some() {
            duplicates += 1;
            UserInterface::warning(&format!(
                "{} v{}: duplicate index entry — the later one replaces the earlier",
                key.0, key.1
            ));
        }
    }

    let mut count = 0;
    let pkg_path = Path::new(pkg_dir);
    if pkg_path.exists() {
        for entry in fs::read_dir(pkg_path)? {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().into_owned();
            if !fname.ends_with(".xcs") {
                continue;
            }
            let Some((pkg_name, version)) = parse_xcs_name(&fname) else {
                UserInterface::warning(&format!(
                    "Skipping '{}': filename does not match <name>-<version>.xcs",
                    fname
                ));
                continue;
            };
            if let Some(&idx) = by_key.get(&(pkg_name.clone(), version.clone())) {
                let file_bytes = fs::read(entry.path())?;
                let mut hasher = Sha256::new();
                hasher.update(&file_bytes);
                let result = hasher.finalize();
                let hash = result
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>();
                index[idx].checksum = Checksum {
                    kind: "sha256".to_string(),
                    value: hash.clone(),
                };
                index[idx].source = format!(
                    "{}/pool/{}/{}/{}.xcs",
                    base_url.trim_end_matches('/'),
                    arch,
                    pkg_name,
                    fname
                );
                count += 1;
                if env::var("OUS_QUIET").is_err() {
                    UserInterface::info(&format!("{} -> sha256={}...", fname, &hash[..16]));
                }
            }
        }
    }

    let json = serde_json::to_string_pretty(&index)?;
    atomic_write(index_file, json.as_bytes())?;
    if duplicates > 0 {
        UserInterface::warning(&format!(
            "Updated {} entries in {} ({} duplicate keys resolved last-wins)",
            count, index_path, duplicates
        ));
    } else {
        UserInterface::success(&format!("Updated {} entries in {}", count, index_path));
    }
    Ok(())
}

pub fn rewrite_source(index_path: &str, base_url: &str, arch: &str) -> Result<()> {
    let index_file = Path::new(index_path);
    if !index_file.exists() {
        return Err(anyhow!("Index not found at {}", index_path));
    }
    let content = fs::read_to_string(index_file)?;
    let mut index: Vec<PackageMetadata> = serde_json::from_str(&content)?;
    if !env::var("OUS_ASSUME_YES").is_ok()
        && !UserInterface::prompt_confirmation(&format!(
            "Rewrite source URLs for {} entries in {}?",
            index.len(),
            index_path
        ))
    {
        return Err(anyhow!("Aborted: source URL rewrite not confirmed"));
    }
    for entry in &mut index {
        let url = format!(
            "{}/pool/{}/{}/{}-{}.xcs",
            base_url.trim_end_matches('/'),
            arch,
            entry.pkg_name,
            entry.pkg_name,
            entry.version
        );
        entry.source = url;
    }
    let json = serde_json::to_string_pretty(&index)?;
    atomic_write(index_file, json.as_bytes())?;
    UserInterface::success(&format!(
        "Rewrote source URLs in {} ({} packages)",
        index_path,
        index.len()
    ));
    Ok(())
}

pub fn sign_packages(index_path: &str, packages_dir: &str, key_id: &str) -> Result<()> {
    let mut failures = 0usize;
    let mut args = vec!["--batch", "--yes", "-u", key_id, "--detach-sign"];
    let index_file = Path::new(index_path);
    if index_file.exists() {
        args.push(index_path);
        let status = Command::new("gpg").args(&args).status()?;
        if !status.success() {
            UserInterface::error(&format!("GPG signing of {} failed", index_path));
            failures += 1;
        } else {
            UserInterface::success(&format!("Signed: {}", index_path));
        }
    }

    let pkg_dir = Path::new(packages_dir);
    if pkg_dir.exists() {
        let mut stack = vec![pkg_dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                let md = fs::symlink_metadata(&path)?;
                if md.is_dir() {
                    stack.push(path);
                } else if md.file_type().is_symlink() {
                    continue;
                } else if path.extension().is_some_and(|e| e == "xcs") {
                    let path_str = path.to_string_lossy().into_owned();
                    let mut sign_args = vec!["--batch", "--yes", "-u", key_id, "--detach-sign"];
                    sign_args.push(path_str.as_str());
                    let status = Command::new("gpg").args(&sign_args).status()?;
                    if status.success() {
                        if env::var("OUS_QUIET").is_err() {
                            UserInterface::info(&format!(
                                "Signed: {}",
                                path.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default()
                            ));
                        }
                    } else {
                        UserInterface::error(&format!("GPG signing of {} failed", path.display()));
                        failures += 1;
                    }
                }
            }
        }
    }

    let pubkey_args = vec!["--batch", "--yes", "-u", key_id, "--export", "--armor"];
    let output = Command::new("gpg").args(&pubkey_args).output()?;
    if output.status.success() {
        fs::write("pubkey.asc", &output.stdout)?;
        UserInterface::success("Exported public key: pubkey.asc");
    }

    if failures > 0 {
        return Err(anyhow!("{} package(s) failed GPG signing", failures));
    }
    Ok(())
}
// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ous-lib-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    // C1: a failing build command must propagate its failure (the old
    // `sh -c "... | tee"` swallowed the exit status of the pipeline head).
    #[test]
    fn test_build_failing_command_status_propagates() {
        let dir = test_dir("build-fail");
        let pkg = Package {
            name: "t".into(),
            version: "1.0".into(),
            source: String::new(),
            build_type: "custom".into(),
            build: vec!["echo hello; exit 3".into()],
            install: vec![],
            dependencies: None,
            links: None,
            arch: "native".into(),
            components: None,
            services: None,
            binaries: None,
            sha256: None,
        };
        let err = build(&pkg, dir.to_str().unwrap()).expect_err("must fail");
        assert!(err.to_string().contains("failed"), "error: {}", err);
        // The combined capture.log is written before failing.
        let log = fs::read_to_string(dir.join("capture.log")).unwrap_or_default();
        assert!(log.contains("hello"), "capture.log missing output");
        let _ = fs::remove_dir_all(&dir);
    }

    // C1: successful commands still capture their log.
    #[test]
    fn test_build_success_captures_log() {
        let dir = test_dir("build-ok");
        let pkg = Package {
            name: "t".into(),
            version: "1.0".into(),
            source: String::new(),
            build_type: "custom".into(),
            build: vec!["echo built-it 1>&2".into()],
            install: vec![],
            dependencies: None,
            links: None,
            arch: "native".into(),
            components: None,
            services: None,
            binaries: None,
            sha256: None,
        };
        let log = build(&pkg, dir.to_str().unwrap()).expect("must succeed");
        assert!(log.contains("built-it"));
        let _ = fs::remove_dir_all(&dir);
    }

    // M1/M2 helpers: force strips the tail steps from resume state.
    #[test]
    fn test_force_strips_tail_steps() {
        let mut state = BuildProgress::default();
        for step in ["fetch", "build", "install", "hash", "metadata", "archive"] {
            state.completed_steps.insert(step.to_string());
        }
        for step in ["archive", "hash", "metadata"] {
            state.completed_steps.remove(step);
        }
        assert!(state.completed_steps.contains("fetch"));
        assert!(state.completed_steps.contains("build"));
        assert!(state.completed_steps.contains("install"));
        assert!(!state.completed_steps.contains("hash"));
        assert!(!state.completed_steps.contains("metadata"));
        assert!(!state.completed_steps.contains("archive"));
    }

    // m4: filename parser variants, including prerelease suffixes.
    #[test]
    fn test_parse_xcs_name_variants() {
        assert_eq!(
            parse_xcs_name("hello-1.0.xcs"),
            Some(("hello".into(), "1.0".into()))
        );
        assert_eq!(
            parse_xcs_name("libfoo-1.0-beta.xcs"),
            Some(("libfoo".into(), "1.0-beta".into()))
        );
        assert_eq!(
            parse_xcs_name("shared-mime-info-2.4rc1.xcs"),
            Some(("shared-mime-info".into(), "2.4rc1".into()))
        );
        assert_eq!(
            parse_xcs_name("openssl-3.2.1.tar.gz.xcs"),
            None,
            "garbage versions must not parse"
        );
        assert_eq!(parse_xcs_name("noversion.xcs"), None);
        assert_eq!(parse_xcs_name("-1.0.xcs"), None, "empty name rejected");
    }

    // m24: natural version ordering in the index sort key.
    #[test]
    fn test_version_sort_key_natural() {
        assert!(version_sort_key("1.10") > version_sort_key("1.9"));
        assert!(version_sort_key("1.2") < version_sort_key("1.10"));
        assert_eq!(version_sort_key("1.0"), version_sort_key("1.0"));
    }

    // from-bit build: a failed build re-run must continue from the first
    // command that did NOT complete, not resubmit already-finished ones.
    #[test]
    fn test_build_resume_skips_completed_commands() {
        // Ensure the resume markers are honoured in this test regardless of
        // any ambient --force/--clean variables in the host environment.
        unsafe {
            std::env::remove_var("OUS_FORCE");
            std::env::remove_var("OUS_CLEAN");
        }
        let dir = test_dir("build-resume");
        let pkg = Package {
            name: "t".into(),
            version: "1.0".into(),
            source: String::new(),
            build_type: "custom".into(),
            build: vec!["touch resume-probe".into(), "exit 3".into()],
            install: vec![],
            dependencies: None,
            links: None,
            arch: "native".into(),
            components: None,
            services: None,
            binaries: None,
            sha256: None,
        };
        // First run: command 1 succeeds (marker written), command 2 fails.
        let first = build(&pkg, dir.to_str().unwrap());
        assert!(first.is_err(), "command 2 must fail");
        assert!(dir.join("resume-probe").exists(), "command 1 ran");
        let marker = build_command_marker(dir.to_str().unwrap(), 0, "touch resume-probe");
        assert!(marker.is_file(), "marker for command 1 must exist");

        // Wipe the side effect, then re-run: command 1 is skipped via its
        // marker, so the probe file must NOT reappear; command 2 fails again.
        let _ = fs::remove_file(dir.join("resume-probe"));
        let second = build(&pkg, dir.to_str().unwrap());
        assert!(second.is_err(), "command 2 still fails");
        assert!(
            !dir.join("resume-probe").exists(),
            "command 1 must be skipped on resume"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // from-bit build: markers are keyed by the command's content hash, so
    // editing a build step in the manifest invalidates its marker.
    #[test]
    fn test_build_marker_keyed_by_command_hash() {
        let dir = test_dir("build-marker-hash");
        let a = build_command_marker(dir.to_str().unwrap(), 2, "make -j8");
        let b = build_command_marker(dir.to_str().unwrap(), 2, "make -j16");
        assert_ne!(a, b, "different commands must map to different markers");
        // Same command always maps to the same marker path.
        let c = build_command_marker(dir.to_str().unwrap(), 2, "make -j8");
        assert_eq!(a, c);
        let _ = fs::remove_dir_all(&dir);
    }

    // m1/m2: component assignment with trailing slashes and overlap.
    #[test]
    fn test_assign_components_trailing_slash_and_overlap() {
        let pkg_files = vec![
            "system/bin/app".to_string(),
            "system/lib/liba.so".to_string(),
            "share/doc/readme".to_string(),
        ];
        let pkg = Package {
            name: "t".into(),
            version: "1.0".into(),
            source: String::new(),
            build_type: "custom".into(),
            build: vec![],
            install: vec![],
            dependencies: None,
            links: None,
            arch: "native".into(),
            components: Some(vec![
                ComponentSpec {
                    name: "runtime".into(),
                    priority: "required".into(),
                    files: vec!["system/".into(), "system/bin/app".into(), "".into()],
                    description: String::new(),
                },
                ComponentSpec {
                    name: "docs".into(),
                    priority: "optional".into(),
                    files: vec!["share/doc/".into()],
                    description: String::new(),
                },
            ]),
            services: None,
            binaries: None,
            sha256: None,
        };
        let comps = assign_components(&pkg_files, &pkg);
        let runtime = comps.iter().find(|c| c.name == "runtime").unwrap();
        let docs = comps.iter().find(|c| c.name == "docs").unwrap();
        // Trailing-slash pattern matched the tree...
        assert!(runtime.files.contains(&PathBuf::from("system/bin/app")));
        assert!(runtime.files.contains(&PathBuf::from("system/lib/liba.so")));
        // ...and first-spec-wins kept it away from the overlapping later spec.
        assert!(!docs.files.contains(&PathBuf::from("system/bin/app")));
        assert!(docs.files.contains(&PathBuf::from("share/doc/readme")));
    }

    // M12: symlink link paths may not escape the staging root.
    #[test]
    fn test_symlink_escape_rejected() {
        let root = test_dir("symlink-root");
        let outside = test_dir("symlink-outside");
        // Traversal via ".."
        assert!(
            symlink(
                "/etc/passwd",
                "../../etc/cron.d/evil",
                root.to_str().unwrap()
            )
            .is_err()
        );
        // A plain valid link still works.
        symlink("../data/blob", "system/share/link", root.to_str().unwrap())
            .expect("valid link must succeed");
        assert!(
            root.join("system/share/link").is_symlink() || root.join("system/share/link").exists()
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    // M12: raw version interpolation into the output filename rejects
    // traversal.
    #[test]
    fn test_version_component_validation() {
        assert!(validate_version_component("1.0-beta", "package version").is_ok());
        assert!(validate_version_component("../evil", "package version").is_err());
        assert!(validate_version_component("a/b", "package version").is_err());
        assert!(validate_version_component("", "package version").is_err());
    }

    // M8: atomic_write renames over existing targets and leaves a valid file.
    #[test]
    fn test_atomic_write_overwrites_existing() {
        let dir = test_dir("atomic");
        let target = dir.join("file.json");
        atomic_write(&target, b"first").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");
        atomic_write(&target, b"second-and-longer").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second-and-longer");
        // No temp litter left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with('.') && n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {:?}", leftovers);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_canonical_arch_mapping() {
        assert_eq!(canonical_arch("x86_64").unwrap(), "x86_64");
        assert_eq!(canonical_arch("aarch64").unwrap(), "aarch64");
        assert_eq!(canonical_arch("native").unwrap(), "native");
        assert_eq!(canonical_arch("amd64").unwrap(), "x86_64");
        assert_eq!(canonical_arch("arm64").unwrap(), "aarch64");
        assert_eq!(canonical_arch("").unwrap(), "native");
        assert_eq!(canonical_arch("  x86_64  ").unwrap(), "x86_64");
    }

    #[test]
    fn test_canonical_arch_rejects_unknown() {
        assert!(canonical_arch("i686").is_err());
        assert!(canonical_arch("riscv64").is_err());
        assert!(canonical_arch("x86_64-unknown-linux-musl").is_err());
        assert!(canonical_arch("arm").is_err());
        let err = canonical_arch("powerpc64").unwrap_err().to_string();
        assert!(
            err.contains("powerpc64"),
            "error must name the rejected value: {}",
            err
        );
    }

    #[test]
    fn test_canonical_arch_used_in_index_metadata() {
        let meta = PackageMetadata {
            pkg_name: "test".into(),
            version: "1.0".into(),
            license: "MIT".into(),
            source: "test".into(),
            arch: "amd64".into(),
            checksum: Checksum {
                kind: "sha256".into(),
                value: "abc".into(),
            },
            dependencies: Vec::new(),
            files: Vec::new(),
            provides: None,
            conflicts: None,
            components: Vec::new(),
            services: Vec::new(),
            binaries: Vec::new(),
            provenance: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(
            json.contains("\"architecture\""),
            "must emit 'architecture' key, got: {}",
            json
        );
        assert!(
            !json.contains("\"arch\""),
            "must NOT emit legacy 'arch' key, got: {}",
            json
        );
    }

    #[test]
    fn test_canonical_arch_metadata_deserializes_legacy_key() {
        let legacy_json = r#"{"pkg_name":"t","version":"1","license":"","source":"","arch":"arm64","checksum":{"kind":"sha256","value":""},"dependencies":[],"files":[]}"#;
        let meta: PackageMetadata = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(meta.arch, "arm64");
    }

    #[test]
    fn test_canonical_arch_metadata_roundtrip_new_key() {
        let meta = PackageMetadata {
            pkg_name: "t".into(),
            version: "1".into(),
            license: "".into(),
            source: "".into(),
            arch: "aarch64".into(),
            checksum: Checksum {
                kind: "sha256".into(),
                value: "".into(),
            },
            dependencies: Vec::new(),
            files: Vec::new(),
            provides: None,
            conflicts: None,
            components: Vec::new(),
            services: Vec::new(),
            binaries: Vec::new(),
            provenance: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: PackageMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, deserialized);
        // Verify new key emitted
        assert!(json.contains("\"architecture\""));
    }

    /// End-to-end archive round-trip: build a small tree, archive it, verify
    /// zstd magic, extract, and confirm metadata.json has the "architecture"
    /// key. Requires `tar` and `zstd` on PATH — ignored by default.
    #[test]
    #[ignore]
    fn test_archive_roundtrip_metadata_architecture_key() {
        let dir = test_dir("archive-rt");
        let pkg_dir = dir.join("pkg");
        let dest_dir = pkg_dir.join("usr").join("bin");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("hello"), b"hello").unwrap();
        fs::write(pkg_dir.join("README"), b"readme").unwrap();

        let metadata = PackageMetadata {
            pkg_name: "roundtrip".into(),
            version: "0.1".into(),
            license: "MIT".into(),
            source: "test".into(),
            arch: "amd64".into(),
            checksum: Checksum {
                kind: "sha256".into(),
                value: "deadbeef".into(),
            },
            dependencies: Vec::new(),
            files: vec![PathBuf::from("usr/bin/hello"), PathBuf::from("README")],
            provides: None,
            conflicts: None,
            components: Vec::new(),
            services: Vec::new(),
            binaries: Vec::new(),
            provenance: None,
        };
        write(&metadata, pkg_dir.to_str().unwrap()).unwrap();

        let xcs = dir.join("roundtrip-0.1.xcs");
        archive(pkg_dir.to_str().unwrap(), xcs.to_str().unwrap()).unwrap();

        // Verify zstd magic (4 bytes: 0x28 0xB5 0x2F 0xFD)
        let mut header = [0u8; 4];
        fs::File::open(&xcs)
            .unwrap()
            .read_exact(&mut header)
            .unwrap();
        assert_eq!(header, [0x28, 0xB5, 0x2F, 0xFD], "not a valid zstd stream");

        // Extract into a fresh tree
        let extract_dir = dir.join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();
        let status = std::process::Command::new("tar")
            .args(["--zstd", "-xf"])
            .arg(&xcs)
            .args(["-C", extract_dir.to_str().unwrap()])
            .status()
            .expect("tar must be available");
        assert!(status.success(), "tar extraction failed");

        // Original files present, metadata.json not expected by consumer
        assert!(extract_dir.join("usr/bin/hello").exists());
        assert!(extract_dir.join("README").exists());

        // metadata.json must have "architecture" (not legacy "arch")
        let meta_content = fs::read_to_string(extract_dir.join("metadata.json")).unwrap();
        assert!(
            meta_content.contains("\"architecture\""),
            "metadata.json must use 'architecture' key: {}",
            meta_content
        );
        assert!(
            !meta_content.contains("\"arch\":"),
            "metadata.json must not use legacy 'arch' key: {}",
            meta_content
        );

        // Verify the architecture value was canonicalized from amd64 -> x86_64
        let parsed: serde_json::Value = serde_json::from_str(&meta_content).unwrap();
        assert_eq!(
            parsed["architecture"].as_str(),
            Some("x86_64"),
            "amd64 must be canonicalized to x86_64"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_source_type_variants() {
        assert_eq!(detect_source_type("/tmp/some/local/dir"), "dir");
        assert_eq!(detect_source_type(""), "dir");
        assert_eq!(detect_source_type("https://example.com/foo.tar.gz"), "http");
        assert_eq!(detect_source_type("http://example.com/foo"), "http");
        assert_eq!(detect_source_type("file:///tmp/foo.tar.xz"), "file");
        assert_eq!(detect_source_type("git@github.com:org/repo.git"), "git");
        assert_eq!(detect_source_type("git://example.com/repo.git"), "git");
        assert_eq!(detect_source_type("https://example.com/repo.git"), "git");
        assert_eq!(detect_source_type("s3://bucket/object"), "url");
    }

    #[test]
    fn test_build_provenance_source_types() {
        let dir = test_dir("prov-src");
        let dir_str = dir.to_str().unwrap().to_string();

        let prov = build_provenance(&dir_str, &dir_str);
        assert_eq!(prov.source_type, "dir");
        assert_eq!(prov.source_url, dir_str);
        assert!(
            prov.source_revision.is_none(),
            "dir sources must have no revision"
        );
        assert!(!prov.built_at.is_empty(), "built_at must be non-empty");
        assert_eq!(prov.builder, "ous-0.7.0");

        let prov = build_provenance("https://example.com/foo.tar.gz", &dir_str);
        assert_eq!(prov.source_type, "http");
        assert_eq!(prov.source_url, "https://example.com/foo.tar.gz");
        assert!(
            prov.source_revision.is_none(),
            "http sources must have no revision"
        );
        assert!(!prov.built_at.is_empty());
        assert_eq!(prov.builder, "ous-0.7.0");

        let prov = build_provenance("file:///tmp/foo.tar.xz", &dir_str);
        assert_eq!(prov.source_type, "file");
        assert!(prov.source_revision.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_git_provenance_revision() {
        let git_check = Command::new("git").arg("--version").output();
        if git_check.is_err() {
            eprintln!("git unavailable; skipping");
            return;
        }
        let repo = test_dir("prov-git");
        let init = Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");
        Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["config", "user.name", "Cudane Tests"])
            .output()
            .unwrap();
        Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["config", "user.email", "tests@cudane.local"])
            .output()
            .unwrap();
        fs::write(repo.join("README"), b"hi").unwrap();
        Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["add", "."])
            .output()
            .unwrap();
        let commit = Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["commit", "-qm", "init", "--allow-empty"])
            .output()
            .unwrap();
        assert!(
            commit.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr)
        );

        let repo_str = repo.to_str().unwrap().to_string();
        let prov = build_provenance(&repo_str, &repo_str);
        assert_eq!(prov.source_type, "dir");
        assert!(
            prov.source_revision.is_none(),
            "local dir sources never carry a revision"
        );

        // A git URL with the materialized dir still yields the revision.
        let prov = build_provenance("https://example.com/repo.git", &repo_str);
        assert_eq!(prov.source_type, "git");
        assert!(prov.source_revision.is_some());

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn test_provenance_written_via_metadata_write_path() {
        let src = test_dir("prov-src");
        let dest = test_dir("prov-dest");
        let repo = test_dir("prov-repo");
        fs::write(src.join("Makefile"), b"all:\n").unwrap();
        fs::create_dir_all(dest.join("usr/bin")).unwrap();
        fs::write(dest.join("usr/bin/hello"), b"hello").unwrap();

        let pkg = Package {
            name: "prov-pkg".into(),
            version: "1.0".into(),
            source: src.to_str().unwrap().to_string(),
            build_type: "manual".into(),
            build: Vec::new(),
            install: Vec::new(),
            dependencies: None,
            links: None,
            arch: "native".into(),
            components: None,
            services: None,
            binaries: None,
            sha256: None,
        };
        let sum = vec![Checksum {
            kind: "sha256".into(),
            value: "abc123".into(),
        }];
        let prov = build_provenance(&pkg.source, src.to_str().unwrap());
        let meta = mtd(
            &pkg,
            dest.to_str().unwrap(),
            &sum,
            src.to_str().unwrap(),
            "",
            &repo,
            Some(prov),
        )
        .unwrap();
        write(&meta, dest.to_str().unwrap()).unwrap();

        let content = fs::read_to_string(dest.join("metadata.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let prov = &value["provenance"];
        assert_eq!(prov["source_type"], "dir");
        assert_eq!(prov["source_url"], pkg.source);
        assert!(
            prov["source_revision"].is_null(),
            "dir sources must emit null revision"
        );
        assert!(!prov["built_at"].as_str().unwrap_or_default().is_empty());
        assert_eq!(prov["builder"], "ous-0.7.0");

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn test_legacy_metadata_without_provenance_deserializes() {
        let legacy = r#"{
            "pkg_name": "legacy",
            "version": "1.0",
            "license": "MIT",
            "source": "old",
            "architecture": "x86_64",
            "checksum": {"kind": "sha256", "value": "abc"},
            "dependencies": [],
            "files": [],
            "provides": null,
            "conflicts": null,
            "components": [],
            "services": [],
            "binaries": []
        }"#;
        let meta: PackageMetadata = serde_json::from_str(legacy).unwrap();
        assert!(
            meta.provenance.is_none(),
            "missing provenance must deserialize to None"
        );

        let with = PackageMetadata {
            pkg_name: "t".into(),
            version: "1".into(),
            license: "".into(),
            source: "".into(),
            arch: "native".into(),
            checksum: Checksum {
                kind: "sha256".into(),
                value: "".into(),
            },
            dependencies: Vec::new(),
            files: Vec::new(),
            provides: None,
            conflicts: None,
            components: Vec::new(),
            services: Vec::new(),
            binaries: Vec::new(),
            provenance: Some(PackageProvenance {
                source_type: "dir".into(),
                source_url: "/tmp/x".into(),
                source_revision: None,
                built_at: "2026-01-01T00:00:00Z".into(),
                builder: "ous-0.7.0".into(),
            }),
        };
        let json = serde_json::to_string(&with).unwrap();
        let back: PackageMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with);
        assert!(json.contains("\"provenance\""));
    }

    #[test]
    fn test_provenance_survives_source_rewrite() {
        let repo = test_dir("prov-rewrite");
        let index_path = repo.join("index.json");
        let meta = PackageMetadata {
            pkg_name: "rew".into(),
            version: "1.0".into(),
            license: "MIT".into(),
            source: "https://example.com/original.tar.gz".into(),
            arch: "x86_64".into(),
            checksum: Checksum {
                kind: "sha256".into(),
                value: "abc".into(),
            },
            dependencies: Vec::new(),
            files: Vec::new(),
            provides: None,
            conflicts: None,
            components: Vec::new(),
            services: Vec::new(),
            binaries: Vec::new(),
            provenance: Some(PackageProvenance {
                source_type: "http".into(),
                source_url: "https://example.com/original.tar.gz".into(),
                source_revision: None,
                built_at: "2026-01-01T00:00:00Z".into(),
                builder: "ous-0.7.0".into(),
            }),
        };
        fs::write(&index_path, serde_json::to_string_pretty(&[meta]).unwrap()).unwrap();
        unsafe { env::set_var("OUS_ASSUME_YES", "1") };
        rewrite_source(
            index_path.to_str().unwrap(),
            "https://repo.example",
            "x86_64",
        )
        .unwrap();
        unsafe { env::remove_var("OUS_ASSUME_YES") };

        let reindex: Vec<PackageMetadata> =
            serde_json::from_str(&fs::read_to_string(&index_path).unwrap()).unwrap();
        assert!(reindex[0].source.starts_with("https://repo.example/pool/"));
        let prov = reindex[0].provenance.as_ref().unwrap();
        assert_eq!(prov.source_url, "https://example.com/original.tar.gz");
        assert_eq!(prov.source_type, "http");

        let _ = fs::remove_dir_all(&repo);
    }

    /// End-to-end: the archive's embedded metadata.json carries the provenance
    /// block. Requires `tar` and `zstd` on PATH — ignored by default.
    #[test]
    #[ignore]
    fn test_provenance_embedded_in_archive_metadata() {
        let dir = test_dir("prov-arch");
        let pkg_dir = dir.join("pkg");
        fs::create_dir_all(pkg_dir.join("usr/bin")).unwrap();
        fs::write(pkg_dir.join("usr/bin/hello"), b"hello").unwrap();

        let metadata = PackageMetadata {
            pkg_name: "provarch".into(),
            version: "0.1".into(),
            license: "MIT".into(),
            source: "https://example.com/hello.tar.gz".into(),
            arch: "native".into(),
            checksum: Checksum {
                kind: "sha256".into(),
                value: "deadbeef".into(),
            },
            dependencies: Vec::new(),
            files: vec![PathBuf::from("usr/bin/hello")],
            provides: None,
            conflicts: None,
            components: Vec::new(),
            services: Vec::new(),
            binaries: Vec::new(),
            provenance: Some(build_provenance("https://example.com/hello.tar.gz", "")),
        };
        write(&metadata, pkg_dir.to_str().unwrap()).unwrap();

        let xcs = dir.join("provarch-0.1.xcs");
        archive(pkg_dir.to_str().unwrap(), xcs.to_str().unwrap()).unwrap();

        let extract_dir = dir.join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();
        let status = std::process::Command::new("tar")
            .args(["--zstd", "-xf"])
            .arg(&xcs)
            .args(["-C", extract_dir.to_str().unwrap()])
            .status()
            .expect("tar must be available");
        assert!(status.success(), "tar extraction failed");

        let content = fs::read_to_string(extract_dir.join("metadata.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["provenance"]["source_type"], "http");
        assert_eq!(
            value["provenance"]["source_url"],
            "https://example.com/hello.tar.gz"
        );
        assert!(
            !value["provenance"]["built_at"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
        assert_eq!(value["provenance"]["builder"], "ous-0.7.0");

        let _ = fs::remove_dir_all(&dir);
    }
}
