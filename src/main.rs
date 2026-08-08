pub mod config;
pub mod utils;
pub mod event;

use crate::utils::ui::UserInterface;

use anyhow::{anyhow, Result};
use ous::{process, Manifest, PackageMetadata};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process as sys_process;
use std::sync::Arc;

struct UiReporter;

impl cps::Reporter for UiReporter {
    fn info(&self, msg: &str) {
        UserInterface::info(msg);
    }
    fn warning(&self, msg: &str) {
        UserInterface::warning(msg);
    }
    fn error(&self, msg: &str) {
        UserInterface::error(msg);
    }
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn main() -> Result<()> {
    cps::configure(cps::Options::new("ous").with_reporter(Arc::new(UiReporter)));

    let mut args = env::args().skip(1).peekable();
    let mut manifest_path = String::new();
    let mut output_dir = String::new();

    if args.len() == 0 {
        utils::UserInterface::info("Usage: ous [OPTIONS] <MANIFEST> <OUTPUT_DIR>\nTry 'ous --help' for more information.");
        sys_process::exit(1);
    }

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("Render Line (Outsider) Build Engine\n");
                utils::UserInterface::info("USAGE:");
                println!("  ous [OPTIONS] <MANIFEST> <OUTPUT_DIR>\n");
                utils::UserInterface::info("BUILD:");
                println!("  ous <manifest> <output_dir>   Build all packages from manifest");
                println!("  -m, --manifest <FILE>        Path to manifest.json");
                println!("  -o, --output <DIR>           Path to output directory");
                println!("  -t, --target <ARCH>          Define target architecture");
                println!("  -n, --no-auto                Disable automatic build/install behaviors");
                println!("  -f, --force                  Overwrite existing .xcs packages");
                println!("  -c, --clean                  Clean workspace before building");
                println!("  -l, --parallel               Enable parallel package processing");
                println!("  -j, --jobs <NUM>             Set number of parallel make jobs");
                println!("  -z, --zstd-level <NUM>       Set zstd compression level (default: 3)");
                println!("  -s, --strict                 Fail immediately on dependency mapping errors");
                println!("  -d, --debug                  Enable verbose debug logging");
                println!("  -q, --quiet                  Suppress non-error output");
                println!("  -y, --yes                    Assume 'yes' to all prompts");
                println!("  -k, --keep-src               Do not delete source directory after build");
                println!("  -p, --project <DIR>          Define custom project/workspace directory");
                utils::UserInterface::info("STANDALONE:");
                println!("  -a, --archive <SRC> <OUT>    Manually archive a directory using tar.zstd (.xcs)");
                println!("  -x, --extract <PKG> <DEST>   Extract standalone package(s) into target rootfs");
                println!("  -w, --write <SRC> <DEST>     Generate metadata.json for directory without archiving");
                println!("  -i, --inspect <PKG>          Inspect package specifications, size, and metadata");
                println!("  -b, --hash-type <TYPE>       Set the checksum algorithm for metadata (default: sha256)");
                utils::UserInterface::info("REPOSITORY:");
                println!("  --sort <DIR> <ARCH>          Sort .xcs files into pool/<arch>/<name>/");
                println!("  --validate <INDEX> <DIR>     Validate index + .xcs file consistency");
                println!("  --checksum <INDEX> <DIR>     Add SHA-256 checksums to index and rewrite source URLs");
                println!("    --base-url <URL>           Base URL for source rewriting (with --checksum)");
                println!("  --source <INDEX>             Rewrite source URLs in index to pool paths");
                println!("    --base-url <URL>           Base URL for source rewriting (with --source)");
                println!("  -g, --sign <INDEX> <DIR>     GPG sign index + all .xcs packages");
                println!("    --key <KEYID>              GPG key ID for signing (with --sign)");
                utils::UserInterface::info("PLUGIN / THEME / TUI:");
                println!("  --plugin list               List registered plugins");
                println!("  --plugin register <name> <path>  Register a new plugin");
                println!("  --plugin unregister <name>  Remove a plugin");
                println!("  --plugin run <name> <func> [args]  Run a plugin function");
                println!("  --theme list                List registered themes");
                println!("  --theme register <name> <path>  Register a theme");
                println!("  --theme unregister <name>   Remove a theme");
                println!("  --theme apply <name>        Apply a theme");
                println!("  --tui list                  List registered TUI apps");
                println!("  --tui register <name> <path>   Register a TUI app");
                println!("  --tui unregister <name>     Remove a TUI app");
                println!("  --tui run <name>            Run a TUI app");
                println!("  -v, --version                Print version information");
                sys_process::exit(0);}
            "-v" | "--version" => {
                println!("Outsider 0.7.0");
                sys_process::exit(0);
            }
            "--plugin" => {
                let sub = args.next().unwrap_or_default();
                match sub.as_str() {
                    "list" => {
                        let plugins = cps::plugin::PluginManager::list();
                        if plugins.is_empty() {
                            utils::UserInterface::info("No plugins registered.");
                        } else {
                            for p in &plugins {
                                let alias_str = if p.aliases.is_empty() { String::new() } else { format!(" [aliases: {}]", p.aliases.keys().cloned().collect::<Vec<_>>().join(", ")) };
                                utils::UserInterface::info(&format!("  {} ({}){}", p.name, p.path, alias_str));
                            }
                        }
                        sys_process::exit(0);
                    }
                    "register" => {
                        let name = args.next().unwrap_or_default();
                        let path = args.next().unwrap_or_default();
                        if name.is_empty() || path.is_empty() {
                            utils::UserInterface::error("Usage: ous --plugin register <name> <path>");
                            sys_process::exit(1);
                        }
                        cps::plugin::PluginManager::register(&name, Path::new(&path), &HashMap::new());
                        utils::UserInterface::success(&format!("Plugin '{}' registered", name));
                        sys_process::exit(0);
                    }
                    "unregister" => {
                        let name = args.next().unwrap_or_default();
                        if name.is_empty() {
                            utils::UserInterface::error("Usage: ous --plugin unregister <name>");
                            sys_process::exit(1);
                        }
                        cps::plugin::PluginManager::unregister(&name);
                        utils::UserInterface::success(&format!("Plugin '{}' unregistered", name));
                        sys_process::exit(0);
                    }
                    "run" => {
                        let name = args.next().unwrap_or_default();
                        let func = args.next().unwrap_or("main".into());
                        let rest: Vec<String> = args.collect();
                        if name.is_empty() {
                            utils::UserInterface::error("Usage: ous --plugin run <name> <func> [args]");
                            sys_process::exit(1);
                        }
                        match cps::plugin::PluginManager::by_name(&name) {
                            Some(entry) => match cps::plugin::PluginManager::run(&entry, &func, &rest) {
                                Ok(out) => { println!("{}", out); }
                                Err(e) => { utils::UserInterface::error(&e); sys_process::exit(1); }
                            },
                            None => { utils::UserInterface::error(&format!("Plugin '{}' not found", name)); sys_process::exit(1); }
                        }
                        sys_process::exit(0);
                    }
                    _ => {
                        utils::UserInterface::error("Usage: ous --plugin <list|register|unregister|run> [args]");
                        sys_process::exit(1);
                    }
                }
            }
            "--theme" => {
                let sub = args.next().unwrap_or_default();
                match sub.as_str() {
                    "list" => {
                        let themes = cps::theme::ThemeEngine::list();
                        if themes.is_empty() {
                            utils::UserInterface::info("No themes registered.");
                        } else {
                            for t in &themes {
                                if t.description.is_empty() {
                                    utils::UserInterface::info(&format!("  {} ({})", t.name, t.path));
                                } else {
                                    utils::UserInterface::info(&format!("  {} ({}) — {}", t.name, t.path, t.description));
                                }
                            }
                        }
                        sys_process::exit(0);
                    }
                    "register" => {
                        let name = args.next().unwrap_or_default();
                        let path = args.next().unwrap_or_default();
                        if name.is_empty() || path.is_empty() {
                            utils::UserInterface::error("Usage: ous --theme register <name> <path>");
                            sys_process::exit(1);
                        }
                        cps::theme::ThemeEngine::register(&name, Path::new(&path));
                        utils::UserInterface::success(&format!("Theme '{}' registered", name));
                        sys_process::exit(0);
                    }
                    "unregister" => {
                        let name = args.next().unwrap_or_default();
                        if name.is_empty() {
                            utils::UserInterface::error("Usage: ous --theme unregister <name>");
                            sys_process::exit(1);
                        }
                        cps::theme::ThemeEngine::unregister(&name);
                        utils::UserInterface::success(&format!("Theme '{}' unregistered", name));
                        sys_process::exit(0);
                    }
                    "apply" => {
                        let name = args.next().unwrap_or_default();
                        if name.is_empty() {
                            utils::UserInterface::error("Usage: ous --theme apply <name>");
                            sys_process::exit(1);
                        }
                        match cps::theme::ThemeEngine::by_name(&name) {
                            Some(entry) => match cps::theme::ThemeEngine::apply(&entry) {
                                Ok(out) => { println!("{}", out); }
                                Err(e) => { utils::UserInterface::error(&e); sys_process::exit(1); }
                            },
                            None => { utils::UserInterface::error(&format!("Theme '{}' not found", name)); sys_process::exit(1); }
                        }
                        sys_process::exit(0);
                    }
                    _ => {
                        utils::UserInterface::error("Usage: ous --theme <list|register|unregister|apply> [args]");
                        sys_process::exit(1);
                    }
                }
            }
            "--tui" => {
                let sub = args.next().unwrap_or_default();
                match sub.as_str() {
                    "list" => {
                        let tuis = cps::tui::TuiEngine::list();
                        if tuis.is_empty() {
                            utils::UserInterface::info("No TUI apps registered.");
                        } else {
                            for t in &tuis {
                                if t.description.is_empty() {
                                    utils::UserInterface::info(&format!("  {} ({})", t.name, t.path));
                                } else {
                                    utils::UserInterface::info(&format!("  {} ({}) — {}", t.name, t.path, t.description));
                                }
                            }
                        }
                        sys_process::exit(0);
                    }
                    "register" => {
                        let name = args.next().unwrap_or_default();
                        let path = args.next().unwrap_or_default();
                        if name.is_empty() || path.is_empty() {
                            utils::UserInterface::error("Usage: ous --tui register <name> <path>");
                            sys_process::exit(1);
                        }
                        cps::tui::TuiEngine::register(&name, Path::new(&path));
                        utils::UserInterface::success(&format!("TUI '{}' registered", name));
                        sys_process::exit(0);
                    }
                    "unregister" => {
                        let name = args.next().unwrap_or_default();
                        if name.is_empty() {
                            utils::UserInterface::error("Usage: ous --tui unregister <name>");
                            sys_process::exit(1);
                        }
                        cps::tui::TuiEngine::unregister(&name);
                        utils::UserInterface::success(&format!("TUI '{}' unregistered", name));
                        sys_process::exit(0);
                    }
                    "run" => {
                        let name = args.next().unwrap_or_default();
                        if name.is_empty() {
                            utils::UserInterface::error("Usage: ous --tui run <name>");
                            sys_process::exit(1);
                        }
                        match cps::tui::TuiEngine::by_name(&name) {
                            Some(entry) => match cps::tui::TuiEngine::apply(&entry) {
                                Ok(out) => { println!("{}", out); }
                                Err(e) => { utils::UserInterface::error(&e); sys_process::exit(1); }
                            },
                            None => { utils::UserInterface::error(&format!("TUI '{}' not found", name)); sys_process::exit(1); }
                        }
                        sys_process::exit(0);
                    }
                    _ => {
                        utils::UserInterface::error("Usage: ous --tui <list|register|unregister|run> [args]");
                        sys_process::exit(1);
                    }
                }
            }
            "-a" | "--archive" => {
                let staging_dir = args.next().into_iter().next().unwrap_or_default();
                let output_package = args.next().into_iter().next().unwrap_or_default();
                if staging_dir.is_empty() || output_package.is_empty() {
                    UserInterface::error("Usage: ous -a <staging_dir> <output_package.xcs>");
                    sys_process::exit(1);
                }
                let level = env::var("OUS_ZSTD_LEVEL").unwrap_or_else(|_| "3".to_string());
                let mut tar_cmd = sys_process::Command::new("tar");
                tar_cmd.args(["-c", "-C", &staging_dir, "."]);
                let mut tar_child = tar_cmd.stdout(sys_process::Stdio::piped()).spawn()?;
                let mut zstd_child = sys_process::Command::new("zstd")
                    .arg(format!("-{}", level))
                    .stdin(sys_process::Stdio::from(tar_child.stdout.take().expect("tar stdout")))
                    .stdout(sys_process::Stdio::from(fs::File::create(&output_package)?))
                    .spawn()?;
                let tar_status = tar_child.wait()?;
                let zstd_status = zstd_child.wait()?;
                if !tar_status.success() || !zstd_status.success() {
                    UserInterface::error("Manual archive compression failed");
                    sys_process::exit(1);
                }
                UserInterface::info(&format!("Successfully archived {} to {}", staging_dir, output_package));
                sys_process::exit(0);
            }
            "-x" | "--extract" => {
                let input_package = args.next().into_iter().next().unwrap_or_default();
                let root = args.next().into_iter().next().unwrap_or_default();
                if input_package.is_empty() || root.is_empty() {
                    UserInterface::error("Usage: ous -x <package.xcs|directory> <root>");
                    sys_process::exit(1);
                }
                fs::create_dir_all(&root)?;
                let path_obj = Path::new(&input_package);
                let mut packages = Vec::new();
                if path_obj.is_dir() {
                    if let Ok(entries) = fs::read_dir(path_obj) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file() && p.extension().is_some_and(|ext| ext == "xcs") {
                                packages.push(p);
                            }
                        }
                    }
                } else {
                    packages.push(path_obj.to_path_buf());
                }
                for f in packages {
                    println!("Unpacking package: {}", f.file_name().expect("path has file name").to_string_lossy());
                    let status = sys_process::Command::new("sh")
                        .args(["-c", &format!("tar --zstd -xf {0} -C {1} 2>/dev/null || zstd -dc {0} | tar -xf - -C {1} 2>/dev/null", sh_quote(&f.to_string_lossy()), sh_quote(&root))])
                        .status()?;
                    if !status.success() {
                        UserInterface::error(&format!("Failed to extract package: {}", f.display()));
                        sys_process::exit(1);
                    }
                }
                sys_process::exit(0);
            }
            "-b" | "--hash-type" => {
                if let Some(val) = args.next() {
                    unsafe { env::set_var("OUS_HASH_TYPE", val) };
                }
            }
            "-w" | "--write" => {
                let src_dir = args.next().into_iter().next().unwrap_or_default();
                let dest_dir = args.next().into_iter().next().unwrap_or_default();
                if src_dir.is_empty() || dest_dir.is_empty() {
                    UserInterface::error("Usage: ous -w <src_dir> <dest_dir>");
                    sys_process::exit(1);
                }
                let mut pkg_name = "custom-package".to_string();
                if let Some(name) = Path::new(&src_dir).file_name() {
                    pkg_name = name.to_string_lossy().into_owned();
                }
                let sum = ous::hash(&dest_dir).unwrap_or_default();
                let repo_root = env::current_dir()?;
                let mock_pkg = ous::Package {
                    name: pkg_name.clone(),
                    version: "manual".into(),
                    source: "manual".into(),
                    build_type: "manual".into(),
                    build_cmd: "".into(),
                    install_cmd: "".into(),
                    links: None,
                    arch: "native".into(),
                    components: None,
                    services: None,
                    binaries: None,
                };
                
                let metadata = match ous::mtd(&mock_pkg, &dest_dir, &sum, &src_dir, "", &repo_root) {
                    Ok(meta) => meta,
                    Err(_) => PackageMetadata {
                        pkg_name,
                        version: "manual".into(),
                        source: "manual".into(),
                        license: "Unknown".into(),
                        arch: "unknown".into(),
                        checksum: sum.first().cloned().unwrap_or(ous::Checksum {
                            kind: "sha256".into(),
                            value: String::new(),
                        }),
                        dependencies: Vec::new(),
                        files: Vec::new(),
                        provides: None,
                        conflicts: None,
                        components: Vec::new(),
                        services: Vec::new(),
                        binaries: Vec::new(),
                    }
                };

                ous::write(&metadata, &dest_dir)?;
                UserInterface::success(&format!("Successfully generated metadata.json inside {}", dest_dir));
                sys_process::exit(0);
            }
            "-i" | "--inspect" => {
                let input_package = args.next().into_iter().next().unwrap_or_default();
                if input_package.is_empty() {
                    UserInterface::error("Usage: os -i <path/to/package.xcs>");
                    sys_process::exit(1);
                }
                
                let path_obj = Path::new(&input_package);
                if !path_obj.exists() {
                    UserInterface::error(&format!("Error: Package file does not exist at '{}'", input_package));
                }

                let sys_meta = fs::metadata(path_obj)?;
                let absolute_path = fs::canonicalize(path_obj).unwrap_or_else(|_| path_obj.to_path_buf());
                
                UserInterface::info("==================================================");
                UserInterface::info("         Outsider Package Inspection Engine          ");
                UserInterface::info("==================================================");
                UserInterface::info(&format!("Package Name/Path : {}", path_obj.file_name().unwrap_or_default().to_string_lossy()));
                UserInterface::info(&format!("Absolute Location: {}", absolute_path.display()));
                UserInterface::info(&format!("Physical Size    : {} bytes ({:.2} MB)", sys_meta.len(), (sys_meta.len() as f64) / 1024.0 / 1024.0));
                
                if let Ok(created_time) = sys_meta.created() {
                    let datetime: chrono::DateTime<chrono::Utc> = created_time.into();
                    println!("Creation Date    : {}", datetime.to_rfc3339());
                } else if let Ok(modified_time) = sys_meta.modified() {
                    let datetime: chrono::DateTime<chrono::Utc> = modified_time.into();
                    println!("Modified Date    : {}", datetime.to_rfc3339());
                }

                let out = sys_process::Command::new("file").arg(path_obj).output()?;
                let ftype = String::from_utf8_lossy(&out.stdout);
                println!("File Type System : {}", ftype.split(':').nth(1).unwrap_or(&ftype).trim());

                println!("\n--- Integrity & Checksum Verification ---");
                let file = fs::File::open(path_obj)?;
                
                let mut embedded: Vec<ous::Checksum> = Vec::new();
                if let Ok(decoder) = zstd::stream::Decoder::new(file) {
                    let mut archive = tar::Archive::new(decoder);
                    if let Ok(entries) = archive.entries() {
                        for entry in entries.flatten() {
                            if let Ok(path) = entry.path() && path.file_name().is_some_and(|n| n == "metadata.json") && let Ok(meta_struct) = serde_json::from_reader::<_, ous::PackageMetadata>(entry) {
                                    embedded = vec![meta_struct.checksum];
                                    break;
                                }
                        }
                    }
                }

                let _file = fs::File::open(path_obj)?;
                let current_checksums = ous::hash(path_obj.to_str().unwrap_or_default())
                    .unwrap_or_default();

                if embedded.is_empty() {
                    UserInterface::warning("Status           : \x1b[33m[WARNING]\x1b[0m Embedded metadata.json not found inside archive.");
                    for cs in &current_checksums {
                        println!("  {}: {}", cs.kind, cs.value);
                    }
                } else {
                    for cs in &embedded {
                        let current = current_checksums.iter().find(|c| c.kind == cs.kind);
                        match current {
                            Some(cur) if cur.value == cs.value => {
                                println!("{}: {} [OK]", cs.kind, cs.value);
                            }
                            Some(cur) => {
                                println!("{}: expected={} actual={} [MISMATCH]", cs.kind, cs.value, cur.value);
                            }
                            None => {
                                println!("{}: {} [missing from file]", cs.kind, cs.value);
                            }
                        }
                    }
                }
                println!("==================================================");

                sys_process::exit(0);
            },
            "--sort" => {
                let dir = args.next().into_iter().next().unwrap_or_default();
                let arch = args.next().into_iter().next().unwrap_or_else(|| "native".to_string());
                if dir.is_empty() {
                    UserInterface::error("Usage: ous --sort <dir> <arch>");
                    sys_process::exit(1);
                }
                ous::sort_packages(&dir, &arch)?;
                sys_process::exit(0);
            }
            "--validate" => {
                let index_path = args.next().into_iter().next().unwrap_or_default();
                let packages_dir = args.next().into_iter().next().unwrap_or_else(|| ".".to_string());
                if index_path.is_empty() {
                    UserInterface::error("Usage: ous --validate <index.json> <packages_dir>");
                    sys_process::exit(1);
                }
                let problems = ous::validate(&index_path, &packages_dir)?;
                sys_process::exit(problems as i32);
            }
            "--checksum" => {
                let index_path = args.next().into_iter().next().unwrap_or_default();
                let pkg_dir = args.next().into_iter().next().unwrap_or_else(|| ".".to_string());
                if index_path.is_empty() {
                    UserInterface::error("Usage: ous --checksum <index.json> <pkg_dir> [--base-url URL] [--arch ARCH]");
                    sys_process::exit(1);
                }
                let mut base_url = env::var("CUDANE_REPO_URL").unwrap_or_else(|_| "https://raw.codeberg.org/Cudane/Repository".to_string());
                let mut arch = env::var("CUDANE_TARGET").unwrap_or_else(|_| "x86_64-unknown-linux-musl".to_string());
                while let Some(next) = args.peek() {
                    if next == "--base-url" {
                        args.next();
                        base_url = args.next().unwrap_or_default();
                    } else if next == "--arch" {
                        args.next();
                        arch = args.next().unwrap_or_default();
                    } else {
                        break;
                    }
                }
                ous::checksum_index(&index_path, &pkg_dir, &base_url, &arch)?;
                sys_process::exit(0);
            }
            "--source" => {
                let index_path = args.next().into_iter().next().unwrap_or_default();
                if index_path.is_empty() {
                    UserInterface::error("Usage: ous --source <index.json> [--base-url URL] [--arch ARCH]");
                    sys_process::exit(1);
                }
                let mut base_url = env::var("CUDANE_REPO_URL").unwrap_or_else(|_| "https://raw.codeberg.org/Cudane/Repository".to_string());
                let mut arch = env::var("CUDANE_TARGET").unwrap_or_else(|_| "x86_64-unknown-linux-musl".to_string());
                while let Some(next) = args.peek() {
                    if next == "--base-url" {
                        args.next();
                        base_url = args.next().unwrap_or_default();
                    } else if next == "--arch" {
                        args.next();
                        arch = args.next().unwrap_or_default();
                    } else {
                        break;
                    }
                }
                ous::rewrite_source(&index_path, &base_url, &arch)?;
                sys_process::exit(0);
            }
            "-g" | "--sign" => {
                let index_path = args.next().into_iter().next().unwrap_or_default();
                let packages_dir = args.next().into_iter().next().unwrap_or_else(|| ".".to_string());
                if index_path.is_empty() {
                    UserInterface::error("Usage: ous --sign <index.json> <packages_dir> --key <KEYID>");
                    sys_process::exit(1);
                }
                let mut key_id = String::new();
                while let Some(next) = args.peek() {
                    if next == "--key" {
                        args.next();
                        key_id = args.next().unwrap_or_default();
                    } else {
                        break;
                    }
                }
                if key_id.is_empty() {
                    key_id = env::var("GPG_KEY_ID").unwrap_or_else(|_| {
                        UserInterface::error("--key <KEYID> or GPG_KEY_ID env var required");
                        sys_process::exit(1);
                    });
                }
                ous::sign_packages(&index_path, &packages_dir, &key_id)?;
                sys_process::exit(0);
            }
            "-n" | "--no-auto" =>
                unsafe { env::set_var("OUS_NO_AUTO", "1")
            },
            "-f" | "--force" =>
                unsafe { env::set_var("OUS_FORCE", "1")
            },
            "-c" | "--clean" =>
                unsafe { env::set_var("OUS_CLEAN", "1")
            },
            "-s" | "--strict" =>
                unsafe { env::set_var("OUS_STRICT", "1")
            },
            "-q" | "--quiet" =>
                unsafe { env::set_var("OUS_QUIET", "1")
            },
            "-d" | "--debug" =>
                unsafe { env::set_var("OUS_DEBUG", "1")
            },
            "-y" | "--yes" =>
                unsafe { env::set_var("OUS_ASSUME_YES", "1")
            },
            "-k" | "--keep-src" =>
                unsafe { env::set_var("OUS_KEEP_SRC", "1")
            },
            "-l" | "--parallel" =>
                unsafe { env::set_var("OUS_PARALLEL", "1")
            },
            "-m" | "--manifest" => {
                if let Some(val) = args.next() {
                    manifest_path = val;
                }
            },
            "-o" | "--output" => {
                if let Some(val) = args.next() {
                    output_dir = val;
                }
            },
            "-j" | "--jobs" => {
                if let Some(val) = args.next() {
                    unsafe {
                        env::set_var("OUS_JOBS", val)
                    };
                }
            },
            "-z" | "--zstd-level" => {
                if let Some(val) = args.next() { 
                    unsafe {
                        env::set_var("OUS_ZSTD_LEVEL", val)
                    };
                }
            },
            "-p" | "--project" => {
                if let Some(val) = args.next() {
                    unsafe {
                        env::set_var("OUS_PROJECT_WORKSPACE", val)
                    };
                }
            },
            "-t" | "--target" => { if let Some(val) = args.next() { unsafe { env::set_var("OUS_TARGET", val) }; } }
            other => {
                if !other.starts_with('-') {
                    if manifest_path.is_empty() {
                        manifest_path = other.to_string();
                    } else if output_dir.is_empty() {
                        output_dir = other.to_string();
                    }
                } else {
                    UserInterface::error(&format!("Unknown argument: {}", other));
                    sys_process::exit(1);
                }
            }
        }
    }

    if manifest_path.is_empty() || output_dir.is_empty() {
        UserInterface::error("Manifest and Output directory are required.");
        UserInterface::info("Usage: ous [OPTIONS] <MANIFEST> <OUTPUT_DIR>\nTry 'ous --help' for more information.");
        sys_process::exit(1);
    }

    let manifest_content = fs::read_to_string(&manifest_path)
        .map_err(|e| {
            UserInterface::error(&format!("Failed to read manifest {}: {}", manifest_path, e));
            anyhow!("Failed to read manifest {}: {}", manifest_path, e)
     })?;
        
    let manifest: Manifest = serde_json::from_str(&manifest_content)
        .map_err(|e| {
            UserInterface::error(&format!("Invalid JSON in manifest: {}", e));
            anyhow!("Invalid JSON in manifest: {}", e)
     })?;

    fs::create_dir_all(&output_dir)
        .map_err(|e| {
            UserInterface::error(&format!("Failed to create output directory {}: {}", output_dir, e));
            anyhow!("Failed to create output directory {}: {}", output_dir, e)
    })?;

    let parallel = env::var("OUS_PARALLEL").is_ok();
    let jobs: usize = env::var("OUS_JOBS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    if parallel && manifest.packages.len() > 1 {
        use std::sync::mpsc;
        use std::thread;

        struct ActiveGuard {
            active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        impl Drop for ActiveGuard {
            fn drop(&mut self) {
                self.active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::new();
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_concurrent = jobs.max(1);

        for pkg in manifest.packages {
            while active.load(std::sync::atomic::Ordering::SeqCst) >= max_concurrent {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let tx = tx.clone();
            let active = std::sync::Arc::clone(&active);
            let out = output_dir.clone();
            let quiet = env::var("OUS_QUIET").is_ok();
            active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            handles.push(thread::spawn(move || {
                let _guard = ActiveGuard { active: std::sync::Arc::clone(&active) };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(&pkg, &out)))
                    .unwrap_or_else(|_| Err(anyhow!("worker panicked")));
                let result = match result {
                    Ok(p) => { if !quiet { format!("OK: {}", p) } else { String::new() } }
                    Err(e) => { format!("Abort: {}", e) }
                };
                let _ = tx.send(result);
            }));
        }
        drop(tx);

        for msg in rx.iter() {
            if msg.starts_with("Abort:") {
                UserInterface::error(&msg);
                sys_process::exit(1);
            } else if !msg.is_empty() && env::var("OUS_QUIET").is_err() {
                UserInterface::success(&msg);
            }
        }
        for h in handles {
            let _ = h.join();
        }
    } else {
        for pkg in &manifest.packages {
            match process(pkg, &output_dir) {
                Ok(p) => {
                    if env::var("OUS_QUIET").is_err() {
                        UserInterface::success(&format!("OK: {}", p));
                    }
                }
                Err(e) => {
                    UserInterface::error(&format!("Abort: {}", e));
                    sys_process::exit(1);
                }
            }
        }
    }
    
    Ok(())
}
