use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json;
#[allow(unused_imports)]
use md5::{Digest as _, Md5};
#[allow(unused_imports)]
use sha1::{Digest as _, Sha1};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, env, fs, io::Read, path::Path, path::PathBuf, process::Command};

pub mod utils;
use crate::utils::ui::UserInterface;

#[derive(Deserialize, Serialize, Clone)]
pub struct Symlink {
    pub target: String,
    pub link: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: String,
    pub build_type: String,
    pub build_cmd: String,
    pub install_cmd: String,
    pub links: Option<std::collections::HashMap<String, String>>,
    #[serde(default = "default_arch")]
    pub arch: String,
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
pub struct PackageMetadata {
    pub pkg_name: String,
    pub version: String,
    pub license: String,
    pub source: String,
    #[serde(default)]
    pub arch: String,
    pub checksum: Checksum,
    pub dependencies: Vec<Dependency>,
    pub files: Vec<PathBuf>,
    pub provides: Option<Vec<String>>,
    pub conflicts: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Manifest {
    pub packages: Vec<Package>,
}

pub fn fetch(src: &str, dir: &str) -> Result<()> {
    let src_path = if let Some(stripped) = src.strip_prefix("file://") { stripped } else { src };

    let src_path_obj = Path::new(src_path);
    if src_path_obj.exists() {
        fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
            fs::create_dir_all(dst)?;
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                let path = entry.path();
                let dest = dst.join(entry.file_name());
                if path.is_dir() {
                    copy_dir_recursive(&path, &dest)?;
                } else {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&path, &dest)?;
                }
            }
            Ok(())
        }

        let dst = Path::new(dir);
        if src_path_obj.is_dir() {
            UserInterface::info("Copying local source directory...");
            copy_dir_recursive(src_path_obj, dst).context("Failed to copy local source directory")?;
        } else {
            UserInterface::info("Copying local source file...");
            fs::create_dir_all(dst)?;
            let file_name = src_path_obj.file_name().ok_or_else(|| anyhow!("Invalid source file name"))?;
            let dest_file = dst.join(file_name);
            fs::copy(src_path_obj, dest_file).context("Failed to copy local source file")?;
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

    let tar_flag = if src.ends_with(".xz") {
        "-xJf"
    } else if src.ends_with(".bz2") {
        "-xjf"
    } else {
        "-xzf"
    };

    UserInterface::info("Extracting source archive...");
    let tar_status = Command::new("tar")
        .args([tar_flag, &archive_str, "-C", dir, "--strip-components=1"])
        .status()?;

    let _ = std::fs::remove_file(&archive_path);

    if !tar_status.success() {
        return Err(anyhow!("Tar failed to decompress archive"));
    }
    
    Ok(())
}

pub fn build(pkg: &Package, dir: &str) -> Result<String> {
    let bcmd = pkg.build_cmd.trim();
    if bcmd.eq_ignore_ascii_case("none") || bcmd.eq_ignore_ascii_case("skip") || bcmd.eq_ignore_ascii_case("nothing") {
        UserInterface::warning("Skipping build step as requested");
        return Ok(String::new());
    }

    if bcmd.is_empty() {
        if std::env::var("OUS_NO_AUTO").is_ok() {
            return Ok(String::new());
        }

        if pkg.build_type == "rust" {
            let target = env::var("OUS_TARGET").unwrap_or_else(|_| "x86_64-unknown-linux-musl".to_string());
            let cpu = if target.contains("aarch64") { "armv8-a" } else { "x86-64-v3" };
            UserInterface::info(&format!("Running automatic cargo build for {target}..."));
            let cmd = format!(
                "RUSTFLAGS=\"-C linker=clang -C target-cpu={cpu} -C opt-level=3 -C lto=fat -C codegen-units=1 -C target-feature=+crt-static -C link-arg=-target -C link-arg={target} -C link-arg=-march={cpu} -C link-arg=-O3 -C link-arg=-flto=full -C link-arg=--sysroot=/system\" cargo build --release --target {target} 2>&1 | tee capture.log"
            );
            let out = Command::new("sh")
                .args(["-c", &cmd])
                .current_dir(dir)
                .output()?;
            let log_content = String::from_utf8_lossy(&out.stdout).to_string();
            if out.status.success() {
                return Ok(log_content);
            } else {
                return Err(anyhow!("Rust auto-build failed: {}", log_content));
            }
        } else {
            return Ok(String::new());
        }
    }

    UserInterface::info("Executing custom build command...");
    let log_file = "capture.log";
    let cmd_with_capture = format!("({}) 2>&1 | tee {}", pkg.build_cmd, log_file);

    let status = Command::new("sh")
        .args(["-c", &cmd_with_capture])
        .current_dir(dir)
        .status()?;

    let log_path = Path::new(dir).join(log_file);
    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);

    if status.success() { Ok(log_content) } else { Err(anyhow!("Build command failed")) }
}

pub fn symlink(target: &str, link_path: &str, root_dir: &str) -> Result<()> {
    let safe_link_path = link_path.trim_start_matches('/');
    let full_link_path = format!("{}/{}", root_dir, safe_link_path);
    
    if let Some(parent) = Path::new(&full_link_path).parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(&full_link_path);
    std::os::unix::fs::symlink(target, &full_link_path)?;
    Ok(())
}

pub fn install(pkg: &Package, src: &str, dest: &str) -> Result<()> {
    let icmd = pkg.install_cmd.trim();
    if icmd.eq_ignore_ascii_case("none") || icmd.eq_ignore_ascii_case("skip") || icmd.eq_ignore_ascii_case("nothing") {
        UserInterface::warning("Skipping install step as requested");
        if let Some(links) = &pkg.links {
            for l in links {
                symlink(&l.0, &l.1, dest)?;
            }
        }
        return Ok(());
    }

    if icmd.is_empty() {
        if std::env::var("OUS_NO_AUTO").is_ok() {
            if let Some(links) = &pkg.links {
                for l in links {
                    symlink(&l.0, &l.1, dest)?;
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
                        if path.is_file() {
                            if let Some(fname) = path.file_name() {
                                let dest_file = Path::new(dest).join(fname);
                                let _ = fs::remove_file(&dest_file);
                                fs::copy(&path, &dest_file)?;
                            }
                        }
                    }
                }
            }

            if let Some(links) = &pkg.links {
                for l in links {
                    symlink(&l.0, &l.1, dest)?;
                }
            }
            return Ok(());
        }
    }

    UserInterface::info("Executing custom install command...");
    let status = Command::new("sh")
        .env("CUDANE_DEST", dest)
        .args(["-c", &pkg.install_cmd])
        .current_dir(src)
        .status()?;

    if !status.success() { return Err(anyhow!("Install command failed")); }

    if let Some(links) = &pkg.links {
        for l in links {
            symlink(&l.0, &l.1, dest)?;
        }
    }
    Ok(())
}

pub fn hash(dir: &str) -> Result<Vec<Checksum>> {
    let output = Command::new("tar").args(["-cf", "-", "-C", dir, "."]).output()?;

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

fn files(dir: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut paths = vec![Path::new(dir).to_path_buf()];
    while let Some(current) = paths.pop() {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    paths.push(path);
                } else {
                    if let Ok(rel) = path.strip_prefix(dir) {
                        files.push(rel.to_string_lossy().to_string());
                    }
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
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".so") || name.contains(".so.") {
                        provides.push(name.to_string());
                    }
                }
            }
        }
    }
    provides.sort();
    provides.dedup();
    Ok(provides)
}

fn license(src_dir: &str) -> String {
    let license_files = ["LICENSE", "COPYING", "LICENSE.MD", "COPYING.MD", "MIT-LICENSE", "UNLICENSE"];
    let license_regex = Regex::new(
        r"(?i)(gnu\s+general\s+public\s+license|gpl|lgpl|agpl|apache|mit|bsd|mpl|mozilla\s+public\s+license|unlicense|isc)\s*(v(?:ersion)?\s*\d+(?:\.\d+)?|\d+[-—]clause|\d+(?:\.\d+)?\b)?"
    ).unwrap();

    if let Ok(entries) = fs::read_dir(src_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_uppercase();
            
            if license_files.iter().any(|&f| name.contains(f)) {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    for cap in license_regex.captures_iter(&content) {
                        let license_name = cap.get(1).map_or("", |m| m.as_str().trim());
                        let mut formatted_name = if license_name.len() <= 4 {
                            license_name.to_uppercase()
                        } else {
                            license_name.split_whitespace()
                                .map(|w| format!("{}{}", &w[..1].to_uppercase(), &w[1..]))
                                .collect::<Vec<String>>()
                                .join(" ")
                        };

                        if let Some(version) = cap.get(2) {
                            let ver_str = version.as_str().trim();
                            if ver_str.to_lowercase().starts_with("v") {
                                let clean_ver = ver_str.trim_start_matches(|c: char| c.is_alphabetic()).trim();
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
                        let cleaned = first_line.trim().trim_matches(|c| c == '*' || c == '#' || c == '/').trim();
                        if !cleaned.is_empty() && cleaned.len() < 60 {
                            return cleaned.to_string();
                        }
                    }
                }
            }
        }
    }
    "Unknown".into()
}

pub fn scan(dest_dir: &str, log_content: &str, current_pkg: &Package, repo_root: &Path) -> Result<Vec<Dependency>> {
    let mut deps_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut pkg_libs: HashMap<String, Vec<String>> = HashMap::new();

    for (name, dep_type) in cdd(log_content) {
        if name.eq_ignore_ascii_case(&current_pkg.name) {
            continue;
        }
        deps_map.entry(name).or_default().insert(dep_type);
    }

    let library_names = libdep(dest_dir)?;
    let library_packages = mltp(repo_root)?;

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
            deps_map.entry(lib).or_default().insert("Library".to_string());
        }
    }

    let index_graph = loadex(repo_root)?;
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

    for line in log_content.lines() {
        let lower = line.to_lowercase();
        let mut extracted_name = String::new();

        if lower.contains("pkg-config") {
            if let Some(caps) = Regex::new(r"(?i)pkg-config[^\n]*--libs\s+([^\s]+)")
                .unwrap()
                .captures(line)
            {
                extracted_name = caps[1].to_string();
            }
        } else if (lower.contains("dependency") || lower.contains("package")) && lower.contains("found") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(idx) = parts.iter().position(|&r| r.eq_ignore_ascii_case("dependency")) {
                if idx + 1 < parts.len() {
                    extracted_name = parts[idx + 1].to_string();
                }
            } else if let Some(idx) = parts.iter().position(|&r| r.eq_ignore_ascii_case("package")) {
                if idx + 1 < parts.len() {
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
            .trim_matches(|c| c == '\'' || c == '"' || c == '`' || c == ':' || c == '.' || c == ',' || c == ';' || c == '(' || c == ')' || c == '[' || c == ']' || c == '{' || c == '}' || c == '/')
            .trim()
            .to_string();

        let ignore_list = ["threads", "for", "pkg-config", "cmake", "ninja", "yes", "no", "module", "function", "program", "library"];
        if !clean_name.is_empty() && !ignore_list.contains(&clean_name.to_lowercase().as_str()) {
            results.push((clean_name, "Build".to_string()));
        }
    }

    results
}

fn libdep(dest_dir: &str) -> Result<HashSet<String>> {
    let mut libs: HashSet<String> = HashSet::new();
    let dest_path = Path::new(dest_dir);
    let mut paths_to_check = vec![dest_path.to_path_buf()];

    while let Some(current_dir) = paths_to_check.pop() {
        if let Ok(entries) = fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    paths_to_check.push(path);
                    continue;
                }

                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".so") || name.ends_with(".dll") || name.ends_with(".dylib") || name.ends_with(".a") {
                        libs.insert(name.to_string());
                    }
                }

                if let Ok(mut f) = fs::File::open(&path) {
                    let mut buf = [0; 4];
                    if f.read_exact(&mut buf).is_ok() && &buf == b"\x7fELF" {
                        if let Ok(out) = Command::new("readelf").arg("-d").arg(&path).output() {
                            let stdout = String::from_utf8_lossy(&out.stdout);
                            for line in stdout.lines() {
                                if line.contains("(NEEDED)") {
                                    if let Some(start) = line.find('[') {
                                        if let Some(end) = line.find(']') {
                                            let lib = line[start + 1..end].to_string();
                                            libs.insert(lib);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(libs)
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

fn mltp(repo_root: &Path) -> Result<HashMap<String, Vec<String>>> {
    let mut library_packages: HashMap<String, Vec<String>> = HashMap::new();
    let ous_root = repo_root.join(".os");

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
            let pkg_root = path.join("pkg");
            if !pkg_root.exists() {
                continue;
            }

            let mut pkg_paths = vec![pkg_root];
            while let Some(current_dir) = pkg_paths.pop() {
                if let Ok(entries) = fs::read_dir(&current_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            pkg_paths.push(path);
                            continue;
                        }

                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.contains(".so") || name.ends_with(".dll") || name.ends_with(".dylib") || name.ends_with(".a") {
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
    }

    Ok(library_packages)
}

fn loadex(repo_root: &Path) -> Result<HashMap<String, HashSet<String>>> {
    let index_path = repo_root.join("index.json");
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();

    if !index_path.exists() {
        return Ok(graph);
    }

    let index_content = fs::read_to_string(&index_path)?;
    let packages: Vec<PackageMetadata> = serde_json::from_str(&index_content).unwrap_or_default();

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

pub fn mtd(pkg: &Package, dest: &str, sum: &[Checksum], src_dir: &str, log_content: &str, repo_root: &Path) -> Result<PackageMetadata> {
    let dependencies = scan(dest, log_content, pkg, repo_root)?;
    let files = files(dest)?;
    let provides = provides(dest)?;

    let target_type = env::var("OUS_HASH_TYPE").unwrap_or_else(|_| "sha256".to_string());
    
    let selected = sum.iter()
        .find(|c| c.kind == target_type)
        .cloned()
        .unwrap_or_else(|| sum[0].clone());

    Ok(PackageMetadata {
        pkg_name: pkg.name.clone(),
        version: pkg.version.clone(),
        license: license(src_dir),
        source: pkg.source.clone(),
        arch: pkg.arch.clone(),
        checksum: selected,
        dependencies,
        files: files.into_iter().map(PathBuf::from).collect(),
        provides: Some(provides), 
        conflicts: None::<Vec<String>>, 
    })
}

pub fn meta(pkg: &Package, dest: &str, sum: &[Checksum], src_dir: &str, log_content: &str) -> Result<()> {
    let repo_root = env::current_dir()?;
    let meta = mtd(pkg, dest, sum, src_dir, log_content, &repo_root)?;
    write(&meta, dest)
}

pub fn write(meta: &PackageMetadata, dest: &str) -> Result<()> {
    let path = format!("{}/metadata.json", dest);
    let json = serde_json::to_string_pretty(meta)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn index(index_root: &str, meta: &PackageMetadata) -> Result<()> {
    let index_name = if meta.arch.is_empty() || meta.arch == "native" {
        "index.json".to_string()
    } else {
        format!("index.{}.json", meta.arch)
    };
    let index_path = Path::new(index_root).join(index_name);
    fs::create_dir_all(index_root)?;

    let mut entries: Vec<PackageMetadata> = if index_path.exists() {
        let existing = fs::read_to_string(&index_path)?;
        serde_json::from_str(&existing).unwrap_or_default()
    } else {
        Vec::new()
    };

    if let Some(existing_meta) = entries.iter_mut().find(|entry| entry.pkg_name == meta.pkg_name && entry.version == meta.version) {
        if *existing_meta == *meta {
            return Ok(());
        }
        *existing_meta = meta.clone();
    } else {
        entries.push(meta.clone());
    }

    entries.sort_by(|a, b| (a.pkg_name.clone(), a.version.clone()).cmp(&(b.pkg_name.clone(), b.version.clone())));
    let json = serde_json::to_string_pretty(&entries)?;
    fs::write(index_path, json)?;
    
    Ok(())
}

pub fn archive(dest: &str, out: &str) -> Result<()> {
    let status = Command::new("sh").args(["-c", &format!("tar -c -C {} . | zstd -3 > {}", dest, out)]).status()?;
    if status.success() { Ok(()) } else { Err(anyhow!("Archive compression failed")) }
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct BuildProgress {
    completed_steps: HashSet<String>,
}

fn save_state(path: &Path, state: &BuildProgress) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn process(pkg: &Package, out_dir: &str) -> Result<String> {
    UserInterface::info(&format!("Processing package: {} v{}", pkg.name, pkg.version));
    let current_dir = env::current_dir()?;

    let absolute_out_dir = current_dir.join(out_dir);
    fs::create_dir_all(&absolute_out_dir)?;

    let final_path = format!("{}/{}-{}.xcs", absolute_out_dir.display(), pkg.name, pkg.version);
    if Path::new(&final_path).exists() && env::var("OUS_FORCE").is_err() {
        UserInterface::success(&format!("Package archive already exists at: {}", final_path));
        return Ok(final_path);
    }

    let arch_dir = if pkg.arch.is_empty() || pkg.arch == "native" {
        pkg.name.clone()
    } else {
        format!("{}/{}", pkg.name, pkg.arch)
    };
    let work_dir = current_dir.join(format!(".os/{}", arch_dir));
    let src_dir = work_dir.join("src");
    let pkg_root = work_dir.join("pkg");
    let state_path = work_dir.join(".state.json");
    let build_log_path = work_dir.join("build_log.txt");
    let sum_path = work_dir.join("checksums.json");

    let mut state = BuildProgress::default();
    let clean = env::var("OUS_CLEAN").is_ok();

    if clean || !state_path.exists() {
        let _ = fs::remove_dir_all(&work_dir);
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&pkg_root)?;
    } else {
        if state_path.exists() {
            state = serde_json::from_str(&fs::read_to_string(&state_path)?).unwrap_or_default();
        }
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&pkg_root)?;
    }

    let src_str = src_dir.to_str().unwrap();
    let root_str = pkg_root.to_str().unwrap();

    if !state.completed_steps.contains("fetch") {
        UserInterface::info("Fetching package source...");
        fetch(&pkg.source, src_str).map_err(|e| {
            UserInterface::error(&format!("Fetch step failed: {}", e));
            anyhow!(e).context("Fetch step failed")
        })?;
        state.completed_steps.insert("fetch".to_string());
        save_state(&state_path, &state)?;
    }

    let build_log = if state.completed_steps.contains("build") {
        fs::read_to_string(&build_log_path).unwrap_or_default()
    } else {
        UserInterface::info("Building package modules...");
        let log = build(pkg, src_str).map_err(|e| {
            UserInterface::error(&format!("Build step failed: {}", e));
            anyhow!(e).context("Build step failed")
        })?;
        fs::write(&build_log_path, &log)?;
        state.completed_steps.insert("build".to_string());
        save_state(&state_path, &state)?;
        log
    };

    if !state.completed_steps.contains("install") {
        UserInterface::info("Installing built files to root target...");
        install(pkg, src_str, root_str).map_err(|e| {
            UserInterface::error(&format!("Install step failed: {}", e));
            anyhow!(e).context("Install step failed")
        })?;
        state.completed_steps.insert("install".to_string());
        save_state(&state_path, &state)?;
    }

    let sum: Vec<Checksum> = if state.completed_steps.contains("hash") {
        serde_json::from_str(&fs::read_to_string(&sum_path)?).unwrap_or_else(|_| {
            hash(root_str).unwrap_or_default()
        })
    } else {
        UserInterface::info("Generating build checksum hash...");
        let s = hash(root_str).map_err(|e| {
            UserInterface::error(&format!("Hashing step failed: {}", e));
            anyhow!(e).context("Hashing step failed")
        })?;
        fs::write(&sum_path, serde_json::to_string(&s)?)?;
        state.completed_steps.insert("hash".to_string());
        save_state(&state_path, &state)?;
        s
    };

    if !state.completed_steps.contains("metadata") {
        UserInterface::info("Compiling dependency graph and manifest metadata...");
        let metadata = mtd(pkg, root_str, &sum, src_str, &build_log, current_dir.as_path()).map_err(|e| {
            UserInterface::error(&format!("Metadata generation failed: {}", e));
            anyhow!(e).context("Metadata generation failed")
        })?;
        write(&metadata, root_str).map_err(|e| {
            UserInterface::error(&format!("Writing metadata json failed: {}", e));
            anyhow!(e).context("Metadata generation failed")
        })?;
        index(current_dir.to_str().unwrap(), &metadata).map_err(|e| {
            UserInterface::error(&format!("Repository index append failed: {}", e));
            anyhow!(e).context("Repository index generation failed")
        })?;
        state.completed_steps.insert("metadata".to_string());
        save_state(&state_path, &state)?;
    }

    if !state.completed_steps.contains("archive") {
        UserInterface::info("Compressing target root into final .xcs package...");
        archive(root_str, &final_path).map_err(|e| {
            UserInterface::error(&format!("Archiving compression failed: {}", e));
            anyhow!(e).context("Archiving step failed")
        })?;
        state.completed_steps.insert("archive".to_string());
        save_state(&state_path, &state)?;
    }

    Ok(final_path)
}