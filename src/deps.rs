use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

// ── ELF parsing (no external binutils required) ──────────────────────────

fn read_at(file: &mut fs::File, offset: u64, len: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let n = file.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

fn vaddr_to_file_offset(segments: &[(u64, u64, u64, u64)], vaddr: u64) -> Option<u64> {
    for &(p_offset, p_vaddr, _p_filesz, p_memsz) in segments {
        if vaddr >= p_vaddr && vaddr < p_vaddr.saturating_add(p_memsz) {
            return Some(p_offset.saturating_add(vaddr - p_vaddr));
        }
    }
    None
}

/// Read the DT_NEEDED library list of an ELF64 file without invoking `readelf`.
/// Returns an empty vector for non-ELF, non-ELFCLASS64, or statically linked
/// files, and an error only when the file cannot be read at all.
pub fn read_elf_needed(path: &Path) -> Result<Vec<String>> {
    let mut file = fs::File::open(path)?;
    let header = read_at(&mut file, 0, 64)?;
    if header.len() < 64 || &header[0..4] != b"\x7fELF" || header[4] != 2 {
        return Ok(Vec::new());
    }

    let phoff = u64::from_le_bytes(header[32..40].try_into()?);
    let phentsize = u16::from_le_bytes(header[54..56].try_into()?);
    let phnum = u16::from_le_bytes(header[56..58].try_into()?);
    if phentsize == 0 || phnum == 0 {
        return Ok(Vec::new());
    }

    let mut segments: Vec<(u64, u64, u64, u64)> = Vec::new();
    let mut dyn_vaddr: Option<u64> = None;
    let mut dyn_size: Option<u64> = None;

    for i in 0..phnum as u64 {
        let ph = read_at(&mut file, phoff.saturating_add(i.saturating_mul(phentsize as u64)), 56)?;
        if ph.len() < 56 {
            break;
        }
        let p_type = u32::from_le_bytes(ph[0..4].try_into()?);
        let p_offset = u64::from_le_bytes(ph[8..16].try_into()?);
        let p_vaddr = u64::from_le_bytes(ph[16..24].try_into()?);
        let p_filesz = u64::from_le_bytes(ph[32..40].try_into()?);
        let p_memsz = u64::from_le_bytes(ph[40..48].try_into()?);
        segments.push((p_offset, p_vaddr, p_filesz, p_memsz));
        if p_type == 2 {
            dyn_vaddr = Some(p_vaddr);
            dyn_size = Some(p_filesz);
        }
    }

    let (dyn_vaddr, dyn_size) = match (dyn_vaddr, dyn_size) {
        (Some(v), Some(s)) => (v, s),
        _ => return Ok(Vec::new()),
    };

    let dyn_off = match vaddr_to_file_offset(&segments, dyn_vaddr) {
        Some(off) => off,
        None => return Ok(Vec::new()),
    };

    let dyn_bytes = read_at(&mut file, dyn_off, dyn_size as usize)?;

    let mut strtab_vaddr: Option<u64> = None;
    let mut strtab_size: Option<u64> = None;
    let mut needed_offsets: Vec<u64> = Vec::new();

    let mut pos = 0;
    while pos + 16 <= dyn_bytes.len() {
        let tag = u64::from_le_bytes(dyn_bytes[pos..pos + 8].try_into()?);
        let val = u64::from_le_bytes(dyn_bytes[pos + 8..pos + 16].try_into()?);
        match tag {
            1 => needed_offsets.push(val),
            5 => strtab_vaddr = Some(val),
            10 => strtab_size = Some(val),
            0 => break,
            _ => {}
        }
        pos += 16;
    }

    let (strtab_vaddr, strtab_size) = match (strtab_vaddr, strtab_size) {
        (Some(v), Some(s)) => (v, s),
        _ => return Ok(Vec::new()),
    };

    let strtab_off = match vaddr_to_file_offset(&segments, strtab_vaddr) {
        Some(off) => off,
        None => return Ok(Vec::new()),
    };

    let strtab = read_at(&mut file, strtab_off, strtab_size as usize)?;

    let mut result = Vec::new();
    for str_off in needed_offsets {
        let off = str_off as usize;
        if off < strtab.len() {
            let end = strtab[off..].iter().position(|&b| b == 0).unwrap_or(strtab.len() - off);
            if end >= 4
                && let Ok(name) = std::str::from_utf8(&strtab[off..off + end]) {
                    result.push(name.to_string());
                }
        }
    }

    Ok(result)
}

fn is_elf(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 4];
    f.read_exact(&mut buf).is_ok() && &buf == b"\x7fELF"
}

/// Scan a directory tree for shared libraries and the ELF DT_NEEDED entries of
/// every executable/library inside it. Falls back to `readelf` only when the
/// in-process parser fails for a given file.
pub fn libdeps(dest_dir: &str) -> Result<HashSet<String>> {
    let mut libs: HashSet<String> = HashSet::new();
    let mut stack = vec![PathBuf::from(dest_dir)];

    while let Some(dir) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }

                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && (name.ends_with(".so") || name.ends_with(".dll") || name.ends_with(".dylib") || name.ends_with(".a")) {
                        libs.insert(name.to_string());
                    }

                if is_elf(&path) {
                    match read_elf_needed(&path) {
                        Ok(needed) => {
                            for lib in needed {
                                libs.insert(lib);
                            }
                        }
                        Err(_) => {
                            if let Ok(out) = Command::new("readelf").arg("-d").arg(&path).output() {
                                let stdout = String::from_utf8_lossy(&out.stdout);
                                for line in stdout.lines() {
                                    if line.contains("(NEEDED)")
                                        && let Some(start) = line.find('[')
                                            && let Some(end) = line.find(']') {
                                                libs.insert(line[start + 1..end].to_string());
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

// ── Source manifest dependency scanning ──────────────────────────────────

type DepSet = HashSet<(String, String)>;

const SKIP_DIRS: &[&str] = &[
    "target", ".git", "node_modules", "vendor", "build", "dist", ".os",
    "__pycache__", ".cargo", "third_party", "deps",
];

/// Walk the unpacked source tree and extract build-time dependency names from
/// the common manifest formats. Fast (pure file reads, no subprocesses) and
/// cheap enough to run on every build.
pub fn scan_source_deps(src_dir: &str) -> Vec<(String, String)> {
    let mut deps: DepSet = HashSet::new();
    let mut stack = vec![PathBuf::from(src_dir)];

    while let Some(dir) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str())
                        && SKIP_DIRS.contains(&name) {
                            continue;
                        }
                    stack.push(path);
                } else if path.is_file() {
                    scan_file(&path, &mut deps);
                }
            }
        }
    }

    let mut result: Vec<(String, String)> = deps.into_iter().collect();
    result.sort();
    result
}

fn scan_file(path: &Path, deps: &mut DepSet) {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    if name == "Cargo.toml" {
        cargo_deps(path, deps);
    } else if name == "package.json" {
        npm_deps(path, deps);
    } else if name == "meson.build" {
        meson_deps(path, deps);
    } else if name == "CMakeLists.txt" || name.ends_with(".cmake") {
        cmake_deps(path, deps);
    } else if name == "configure.ac" || name == "configure" {
        autotools_deps(path, deps);
    } else if name.ends_with(".pc") {
        pkgconfig_deps(path, deps);
    }
}

fn cargo_deps(path: &Path, deps: &mut DepSet) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let Ok(toml_value) = content.parse::<toml::Value>() else {
        return;
    };

    let mut tables: Vec<Option<&toml::Table>> = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        tables.push(toml_value.get(section).and_then(|v| v.as_table()));
    }
    if let Some(workspace) = toml_value.get("workspace").and_then(|v| v.as_table())
        && let Some(ws_deps) = workspace.get("dependencies").and_then(|v| v.as_table()) {
            tables.push(Some(ws_deps));
        }

    for table in tables.into_iter().flatten() {
        for key in table.keys() {
            deps.insert((key.clone(), "Build (cargo)".to_string()));
        }
    }
}

fn npm_deps(path: &Path, deps: &mut DepSet) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    for section in ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(obj) = value.get(section).and_then(|v| v.as_object()) {
            for key in obj.keys() {
                deps.insert((key.clone(), "Build (npm)".to_string()));
            }
        }
    }
}

fn meson_deps(path: &Path, deps: &mut DepSet) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let re = Regex::new(r#"dependency\s*\(\s*['"]([^'"]+)['"]"#).expect("valid regex");
    for caps in re.captures_iter(&content) {
        deps.insert((caps[1].to_string(), "Build (meson)".to_string()));
    }
}

fn cmake_deps(path: &Path, deps: &mut DepSet) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let re = Regex::new(r"(?i)find_package\s*\(\s*([A-Za-z0-9_+-]+)").expect("valid regex");
    for caps in re.captures_iter(&content) {
        deps.insert((caps[1].to_string(), "Build (cmake)".to_string()));
    }
}

fn autotools_deps(path: &Path, deps: &mut DepSet) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let pkgconfig_re = Regex::new(
        r#"(?i)PKG_CHECK_MODULES\s*\(\s*[^,]+,\s*['"]?([A-Za-z0-9_+\-./]+)['"]?"#,
    ).expect("valid regex");
    for caps in pkgconfig_re.captures_iter(&content) {
        deps.insert((caps[1].to_string(), "Build (pkg-config)".to_string()));
    }

    let check_lib_re = Regex::new(
        r#"(?i)AC_CHECK_LIB\s*\(\s*['"]?([A-Za-z0-9_+-]+)['"]?"#,
    ).expect("valid regex");
    for caps in check_lib_re.captures_iter(&content) {
        deps.insert((caps[1].to_string(), "Build (autotools)".to_string()));
    }
}

fn pkgconfig_deps(path: &Path, deps: &mut DepSet) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let trimmed = line.trim();
        let rest = trimmed
            .strip_prefix("Requires.private:")
            .or_else(|| trimmed.strip_prefix("Requires:"))
            .or_else(|| trimmed.strip_prefix("requires.private:"))
            .or_else(|| trimmed.strip_prefix("requires:"));
        if let Some(rest) = rest {
            for tok in rest.split_whitespace() {
                let name = tok
                    .split(['>', '<', '=', '(', ')', '!'])
                    .next()
                    .unwrap_or(tok)
                    .trim();
                if !name.is_empty() && name != "and" && name != "or" {
                    deps.insert((name.to_string(), "Build (pkg-config)".to_string()));
                }
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkgconfig_requires_parsing() {
        let dir = std::env::temp_dir().join(format!("ous-dep-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("foo.pc"),
            "Requires: glib-2.0 >= 2.50 zlib\nRequires.private: openssl\n",
        )
        .unwrap();
        let mut deps: DepSet = HashSet::new();
        pkgconfig_deps(&dir.join("foo.pc"), &mut deps);
        let names: Vec<String> = deps.into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"glib-2.0".to_string()));
        assert!(names.contains(&"zlib".to_string()));
        assert!(names.contains(&"openssl".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_meson_dependency_parsing() {
        let dir = std::env::temp_dir().join(format!("ous-meson-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("meson.build"),
            "project('x')\ndep = dependency('libpcre2-8', version: '>=10.0')\ndep2 = dependency('zlib')\n",
        )
        .unwrap();
        let mut deps: DepSet = HashSet::new();
        meson_deps(&dir.join("meson.build"), &mut deps);
        let names: Vec<String> = deps.into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"libpcre2-8".to_string()));
        assert!(names.contains(&"zlib".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}
