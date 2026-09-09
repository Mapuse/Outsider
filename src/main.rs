use anyhow::{Result, anyhow};
use ous::utils::ui::UserInterface;
use ous::{Manifest, PackageMetadata, canonical_arch, process};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Load ous.toml and expose its settings through the OUS_* environment
/// variables the pipeline reads. CLI flags and pre-set env vars win.
fn apply_config_defaults() -> ous::config::schema::Config {
    let cfg = ous::config::loader::load();
    let set_if_unset = |key: &str, value: String| {
        if env::var_os(key).is_none() {
            unsafe {
                env::set_var(key, value);
            }
        }
    };
    if cfg.general.quiet {
        set_if_unset("OUS_QUIET", "1".into());
    }
    if cfg.general.debug {
        set_if_unset("OUS_DEBUG", "1".into());
    }
    if cfg.build.zstd_level != 3 {
        set_if_unset("OUS_ZSTD_LEVEL", cfg.build.zstd_level.to_string());
    }
    if cfg.build.target_arch != ous::config::schema::default_target_arch() {
        set_if_unset("OUS_TARGET", cfg.build.target_arch.clone());
    }
    if cfg.build.parallel {
        set_if_unset("OUS_PARALLEL", "1".into());
    }
    if cfg.build.jobs > 1 {
        set_if_unset("OUS_JOBS", cfg.build.jobs.to_string());
    }
    if cfg.build.force {
        set_if_unset("OUS_FORCE", "1".into());
    }
    if cfg.build.clean {
        set_if_unset("OUS_CLEAN", "1".into());
    }
    if cfg.build.no_auto {
        set_if_unset("OUS_NO_AUTO", "1".into());
    }
    if cfg.build.hash_type != "sha256" {
        set_if_unset("OUS_HASH_TYPE", cfg.build.hash_type.clone());
    }
    cfg
}

/// Number of value tokens a flag consumes during argv preprocessing.
fn value_flag_arity(flag: &str) -> Option<usize> {
    Some(match flag {
        "-a" | "--archive" | "-x" | "--extract" | "-w" | "--write" => 2,
        "-i" | "--inspect" | "-m" | "--manifest" | "-o" | "--output" | "-j" | "--jobs" | "-z"
        | "--zstd-level" | "-p" | "--project" | "-t" | "--target" | "-b" | "--hash-type"
        | "-u" | "--upload" | "--token" | "--sort" | "--validate" | "--checksum" | "--sign"
        | "--source" | "--plugin" | "--theme" | "--tui" | "--base-url" | "--arch" | "--key" => 1,
        _ => return None,
    })
}

/// Preprocess raw argv into a dispatch-ready sequence. Repository modifier
/// options (`--base-url`/`--arch`/`--key`) are hoisted to the front so both
/// the leading form (`--base-url X --checksum idx dir`) and the trailing form
/// (`--checksum idx dir --base-url X`) parse identically; every other token
/// keeps its original relative order.
fn preprocess_argv(argv: &[String]) -> Vec<String> {
    let mut hoisted: Vec<String> = Vec::new();
    let mut ordered: Vec<String> = Vec::new();

    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].clone();
        if a.starts_with('-') && a.len() > 1 {
            let mut unit = vec![a.clone()];
            if let Some(n) = value_flag_arity(&a) {
                for _ in 0..n {
                    if i + 1 < argv.len() {
                        i += 1;
                        unit.push(argv[i].clone());
                    }
                }
            }
            if matches!(a.as_str(), "--base-url" | "--arch" | "--key") {
                hoisted.extend(unit);
            } else {
                ordered.extend(unit);
            }
        } else {
            ordered.push(a);
        }
        i += 1;
    }

    // Hoisted modifiers go first so they are stashed before any repository
    // command consumes them; everything else stays in source order.
    let mut result = hoisted;
    result.extend(ordered);
    result
}

/// Resolve the repository-layout arch from a --arch flag / env var / default.
fn resolve_repo_arch(opt_arch: &Option<String>) -> Result<String> {
    let raw = opt_arch
        .clone()
        .or_else(|| env::var("CUDANE_TARGET").ok())
        .unwrap_or_else(|| "native".to_string());
    canonical_arch(&raw)
}

fn print_help() {
    println!("Render Line (Outsider) Build Engine\n");
    UserInterface::info("USAGE:");
    println!("  ous [OPTIONS] <MANIFEST> <OUTPUT_DIR>\n");
    UserInterface::info("BUILD:");
    println!("  ous <manifest> <output_dir>   Build all packages from manifest");
    println!("  -m, --manifest <FILE>        Path to manifest.json");
    println!("  -o, --output <DIR>           Path to output directory");
    println!("  -t, --target <ARCH>          Define target architecture");
    println!("  -n, --no-auto                Disable automatic build/install behaviors");
    println!("  -f, --force                  Overwrite existing .xcs packages (rebuilds archive)");
    println!("  -c, --clean                  Clean workspace before building");
    println!("  -l, --parallel               Enable parallel package processing");
    println!("  -j, --jobs <NUM>             Set number of parallel make jobs");
    println!("  -z, --zstd-level <NUM>       Set zstd compression level (default: 3)");
    println!("  -s, --strict                 Fail when libraries cannot be resolved to packages");
    println!("  -d, --debug                  Enable verbose debug logging");
    println!("  -q, --quiet                  Suppress non-error output");
    println!("  -y, --yes                    Assume 'yes' to all confirmation prompts");
    println!("  -p, --project <DIR>          Define custom project/workspace directory");
    UserInterface::info("STANDALONE:");
    println!("  -a, --archive <SRC> <OUT>    Manually archive a directory using tar.zstd (.xcs)");
    println!("  -x, --extract <PKG> <DEST>   Extract standalone package(s) into target rootfs");
    println!(
        "  -w, --write <SRC> <DEST>     Generate metadata.json for directory without archiving"
    );
    println!("  -i, --inspect <PKG>          Inspect package specifications, size, and metadata");
    println!(
        "  -b, --hash-type <TYPE>       Set the checksum algorithm (sha256|sha1|md5, default: sha256)"
    );
    UserInterface::info("REPOSITORY:");
    println!("  --sort <DIR> <ARCH>          Sort .xcs files into pool/<arch>/<name>/");
    println!("  --validate <INDEX> <DIR>     Validate index + .xcs file consistency");
    println!(
        "  --checksum <INDEX> <DIR>     Add SHA-256 checksums to index and rewrite source URLs"
    );
    println!("    --base-url <URL>           Base URL for source rewriting (with --checksum)");
    println!("  --source <INDEX>             Rewrite source URLs in index to pool paths");
    println!("    --base-url <URL>           Base URL for source rewriting (with --source)");
    println!("  -g, --sign <INDEX> <DIR>     GPG sign index + all .xcs packages");
    println!("    --key <KEYID>              GPG key ID for signing (with --sign)");
    println!("  -u, --upload <URL>           Upload built .xcs + sidecar (+ index) by HTTP PUT");
    println!("    --token <SECRET>           Bearer token for upload authorization");
    println!("    --upload-index             Also upload the updated index.<arch>.json");
    UserInterface::info("PLUGIN / THEME / TUI:");
    println!("  --plugin list               List registered plugins");
    println!("  --plugin register|unregister  NOT SUPPORTED: edit p.desc instead");
    println!("  --plugin run <name> <func> [args]  Run a plugin function");
    println!("  --theme list                List registered themes");
    println!("  --theme register|unregister   NOT SUPPORTED: edit t.desc instead");
    println!("  --theme apply <name>        Apply a theme");
    println!("  --tui list                  List registered TUI apps");
    println!("  --tui register|unregister     NOT SUPPORTED: edit t.desc instead");
    println!("  --tui run <name>            Run a TUI app");
    println!("  -v, --version                Print version information");
}

fn not_supported(what: &str, desc_file: &str) -> ! {
    UserInterface::error(&format!(
        "{} register/unregister is not supported: descriptors are managed by editing {}",
        what, desc_file
    ));
    sys_process::exit(1);
}

/// Format a SystemTime as an ISO-8601 UTC timestamp without external crates.
fn iso8601_utc(system_time: std::time::SystemTime) -> String {
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
    let dur = system_time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
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

fn main() -> Result<()> {
    cps::configure(cps::Options::new("ous").with_reporter(Arc::new(UiReporter)));
    let cfg = apply_config_defaults();

    let raw_args: Vec<String> = env::args().skip(1).collect();
    if raw_args.is_empty() {
        UserInterface::info(
            "Usage: ous [OPTIONS] <MANIFEST> <OUTPUT_DIR>\nTry 'ous --help' for more information.",
        );
        sys_process::exit(1);
    }

    let args = preprocess_argv(&raw_args);
    let mut idx = 0usize;
    let mut manifest_path = String::new();
    let mut output_dir = String::new();

    // Repository modifier stash (--base-url/--arch/--key are hoisted to the
    // front of argv by preprocess_argv, so these are always populated before
    // any repository command runs).
    let mut opt_base_url: Option<String> = None;
    let mut opt_arch: Option<String> = None;
    let mut opt_key: Option<String> = None;

    while idx < args.len() {
        let arg = args[idx].clone();
        idx += 1;
        let mut take_value = || -> Option<String> {
            if idx < args.len() {
                let v = args[idx].clone();
                idx += 1;
                Some(v)
            } else {
                None
            }
        };

        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-v" | "--version" => {
                println!("ous {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--base-url" => {
                opt_base_url = args.get(idx).cloned();
                idx += 1;
            }
            "--arch" => {
                opt_arch = args.get(idx).cloned();
                idx += 1;
            }
            "--key" => {
                opt_key = args.get(idx).cloned();
                idx += 1;
            }
            "--plugin" => {
                let sub = take_value().unwrap_or_default();
                match sub.as_str() {
                    "list" => {
                        let plugins = cps::plugin::PluginManager::list();
                        if plugins.is_empty() {
                            UserInterface::info("No plugins registered.");
                        } else {
                            for p in &plugins {
                                UserInterface::info(&format!("  {} ({})", p.name, p.path));
                            }
                        }
                        sys_process::exit(0);
                    }
                    "register" | "unregister" => not_supported("plugin", "p.desc"),
                    "run" => {
                        ous::init_plugins(&cfg.python)?;
                        let name = take_value().unwrap_or_default();
                        let func = take_value().unwrap_or_default();
                        let rest: Vec<String> = args[idx..].to_vec();
                        if name.is_empty() || func.is_empty() {
                            UserInterface::error("Usage: ous --plugin run <name> <func> [args]");
                            sys_process::exit(1);
                        }
                        match cps::plugin::PluginManager::by_name(&name) {
                            Some(entry) => {
                                match cps::plugin::PluginManager::run(&entry, &func, &rest) {
                                    Ok(out) => println!("{}", out),
                                    Err(e) => {
                                        UserInterface::error(&e);
                                        sys_process::exit(1);
                                    }
                                }
                            }
                            None => {
                                UserInterface::error(&format!("Plugin '{}' not found", name));
                                sys_process::exit(1);
                            }
                        }
                        sys_process::exit(0);
                    }
                    "reload" => {
                        ous::init_plugins(&cfg.python)?;
                        UserInterface::success("Plugins reloaded");
                        sys_process::exit(0);
                    }
                    _ => {
                        UserInterface::error(
                            "Usage: ous --plugin <list|register|unregister|run|reload> [args]",
                        );
                        sys_process::exit(1);
                    }
                }
            }
            "--theme" => {
                let sub = take_value().unwrap_or_default();
                match sub.as_str() {
                    "list" => {
                        let themes = cps::theme::ThemeEngine::list();
                        if themes.is_empty() {
                            UserInterface::info("No themes registered.");
                        } else {
                            for t in &themes {
                                if t.description.is_empty() {
                                    UserInterface::info(&format!("  {} ({})", t.name, t.path));
                                } else {
                                    UserInterface::info(&format!(
                                        "  {} ({}) — {}",
                                        t.name, t.path, t.description
                                    ));
                                }
                            }
                        }
                        sys_process::exit(0);
                    }
                    "register" | "unregister" => not_supported("theme", "t.desc"),
                    "apply" => {
                        let name = take_value().unwrap_or_default();
                        if name.is_empty() {
                            UserInterface::error("Usage: ous --theme apply <name>");
                            sys_process::exit(1);
                        }
                        match cps::theme::ThemeEngine::by_name(&name) {
                            Some(entry) => match cps::theme::ThemeEngine::apply(&entry) {
                                Ok(out) => println!("{}", out),
                                Err(e) => {
                                    UserInterface::error(&e);
                                    sys_process::exit(1);
                                }
                            },
                            None => {
                                UserInterface::error(&format!("Theme '{}' not found", name));
                                sys_process::exit(1);
                            }
                        }
                        sys_process::exit(0);
                    }
                    _ => {
                        UserInterface::error(
                            "Usage: ous --theme <list|register|unregister|apply> [args]",
                        );
                        sys_process::exit(1);
                    }
                }
            }
            "--tui" => {
                let sub = take_value().unwrap_or_default();
                match sub.as_str() {
                    "list" => {
                        let tuis = cps::tui::TuiEngine::list();
                        if tuis.is_empty() {
                            UserInterface::info("No TUI apps registered.");
                        } else {
                            for t in &tuis {
                                if t.description.is_empty() {
                                    UserInterface::info(&format!("  {} ({})", t.name, t.path));
                                } else {
                                    UserInterface::info(&format!(
                                        "  {} ({}) — {}",
                                        t.name, t.path, t.description
                                    ));
                                }
                            }
                        }
                        sys_process::exit(0);
                    }
                    "register" | "unregister" => not_supported("tui", "t.desc"),
                    "run" => {
                        let name = take_value().unwrap_or_default();
                        if name.is_empty() {
                            UserInterface::error("Usage: ous --tui run <name>");
                            sys_process::exit(1);
                        }
                        match cps::tui::TuiEngine::by_name(&name) {
                            Some(entry) => match cps::tui::TuiEngine::apply(&entry) {
                                Ok(out) => println!("{}", out),
                                Err(e) => {
                                    UserInterface::error(&e);
                                    sys_process::exit(1);
                                }
                            },
                            None => {
                                UserInterface::error(&format!("TUI '{}' not found", name));
                                sys_process::exit(1);
                            }
                        }
                        sys_process::exit(0);
                    }
                    _ => {
                        UserInterface::error(
                            "Usage: ous --tui <list|register|unregister|run> [args]",
                        );
                        sys_process::exit(1);
                    }
                }
            }
            "-a" | "--archive" => {
                let staging_dir = take_value().unwrap_or_default();
                let output_package = take_value().unwrap_or_default();
                if staging_dir.is_empty() || output_package.is_empty() {
                    UserInterface::error("Usage: ous -a <staging_dir> <output_package.xcs>");
                    sys_process::exit(1);
                }
                ous::archive(&staging_dir, &output_package)?;
                UserInterface::info(&format!(
                    "Successfully archived {} to {}",
                    staging_dir, output_package
                ));
                sys_process::exit(0);
            }
            "-x" | "--extract" => {
                let input_package = take_value().unwrap_or_default();
                let root = take_value().unwrap_or_default();
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
                    println!(
                        "Unpacking package: {}",
                        f.file_name().expect("path has file name").to_string_lossy()
                    );

                    // Primary path: a single tar invocation, with stderr kept
                    // so failures are diagnosable.
                    let direct = sys_process::Command::new("tar")
                        .args(["--zstd", "-xf"])
                        .arg(&f)
                        .args(["-C", &root])
                        .stderr(sys_process::Stdio::piped())
                        .output()?;
                    if direct.status.success() {
                        continue;
                    }

                    // Fallback: zstd | tar pipeline (older tar without --zstd).
                    UserInterface::warning(
                        "Direct extraction failed; falling back to 'zstd | tar' pipeline",
                    );
                    let pipeline = sys_process::Command::new("sh")
                        .args(["-c", "zstd -dc \"$1\" | tar -xf - -C \"$2\""])
                        .arg("ous-extract")
                        .arg(&f)
                        .arg(&root)
                        .status()?;
                    if !pipeline.success() {
                        // Identify which stage failed so the message is actionable.
                        let probe = sys_process::Command::new("sh")
                            .args(["-c", "zstd -dc \"$1\" > /dev/null"])
                            .arg("ous-probe")
                            .arg(&f)
                            .status()?;
                        if !probe.success() {
                            UserInterface::error(&format!(
                                "Archive is not a valid zstd stream: {}",
                                f.display()
                            ));
                        } else {
                            UserInterface::error(&format!(
                                "tar failed to unpack the decompressed stream of {} into {}",
                                f.display(),
                                root
                            ));
                        }
                        sys_process::exit(1);
                    }
                }
                sys_process::exit(0);
            }
            "-b" | "--hash-type" => {
                let val = take_value().unwrap_or_default();
                let normalized = val.to_lowercase();
                if matches!(normalized.as_str(), "sha256" | "sha1" | "md5") {
                    unsafe { env::set_var("OUS_HASH_TYPE", &normalized) };
                } else {
                    UserInterface::error(&format!(
                        "Unsupported hash type '{}': allowed values are sha256, sha1, md5",
                        val
                    ));
                    sys_process::exit(2);
                }
            }
            "-w" | "--write" => {
                let src_dir = take_value().unwrap_or_default();
                let dest_dir = take_value().unwrap_or_default();
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
                let prov = ous::build_provenance(&mock_pkg.source, &src_dir);

                let metadata = match ous::mtd(
                    &mock_pkg,
                    &dest_dir,
                    &sum,
                    &src_dir,
                    "",
                    &repo_root,
                    Some(prov),
                ) {
                    Ok(meta) => meta,
                    Err(e) => {
                        // Surface why metadata generation fell back to the skeleton.
                        UserInterface::error(&format!(
                            "Metadata generation failed ({}); writing minimal metadata.json",
                            e
                        ));
                        PackageMetadata {
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
                            provenance: Some(ous::build_provenance("manual", &src_dir)),
                        }
                    }
                };

                ous::write(&metadata, &dest_dir)?;
                UserInterface::success(&format!(
                    "Successfully generated metadata.json inside {}",
                    dest_dir
                ));
                sys_process::exit(0);
            }
            "-i" | "--inspect" => {
                let input_package = take_value().unwrap_or_default();
                if input_package.is_empty() {
                    UserInterface::error("Usage: ous -i <path/to/package.xcs>");
                    sys_process::exit(1);
                }
                let path_obj = Path::new(&input_package);
                if !path_obj.exists() {
                    UserInterface::error(&format!(
                        "Error: Package file does not exist at '{}'",
                        input_package
                    ));
                    sys_process::exit(1);
                }

                let sys_meta = fs::metadata(path_obj)?;
                let absolute_path =
                    fs::canonicalize(path_obj).unwrap_or_else(|_| path_obj.to_path_buf());

                UserInterface::info("==================================================");
                UserInterface::info("         Outsider Package Inspection Engine          ");
                UserInterface::info("==================================================");
                UserInterface::info(&format!(
                    "Package Name/Path : {}",
                    path_obj.file_name().unwrap_or_default().to_string_lossy()
                ));
                UserInterface::info(&format!("Absolute Location: {}", absolute_path.display()));
                UserInterface::info(&format!(
                    "Physical Size    : {} bytes ({:.2} MB)",
                    sys_meta.len(),
                    (sys_meta.len() as f64) / 1024.0 / 1024.0
                ));

                if let Ok(created_time) = sys_meta.created() {
                    println!("Creation Date    : {}", iso8601_utc(created_time));
                } else if let Ok(modified_time) = sys_meta.modified() {
                    println!("Modified Date    : {}", iso8601_utc(modified_time));
                }

                let out = sys_process::Command::new("file").arg(path_obj).output()?;
                let ftype = String::from_utf8_lossy(&out.stdout);
                println!(
                    "File Type System : {}",
                    ftype.split(':').nth(1).unwrap_or(&ftype).trim()
                );

                println!("\n--- Integrity & Checksum Verification ---");
                let current_checksums = ous::hash_file(path_obj).unwrap_or_default();

                // Integrity is verified against the detached sidecar written at
                // packaging time (<package>.sha256), not against metadata.json
                // embedded in the archive (which an attacker could rewrite).
                let sidecar = PathBuf::from(format!("{}.sha256", path_obj.display()));
                match fs::read_to_string(&sidecar) {
                    Ok(content) => {
                        for line in content.lines() {
                            let mut parts = line.split_whitespace();
                            let (Some(expected), _) = (parts.next(), parts.next()) else {
                                continue;
                            };
                            let actual = current_checksums
                                .iter()
                                .find(|c| c.kind == "sha256")
                                .map(|c| c.value.clone())
                                .unwrap_or_default();
                            if actual == expected {
                                println!("sha256: {} [OK]", expected);
                            } else {
                                println!(
                                    "sha256: expected={} actual={} [MISMATCH]",
                                    expected, actual
                                );
                            }
                        }
                    }
                    Err(_) => {
                        UserInterface::warning(&format!(
                            "Status           : [WARNING] Sidecar {}.sha256 not found; showing computed hashes only.",
                            path_obj.display()
                        ));
                        for cs in &current_checksums {
                            println!("  {}: {}", cs.kind, cs.value);
                        }
                    }
                }
                println!("==================================================");

                sys_process::exit(0);
            }
            "--sort" => {
                let dir = take_value().unwrap_or_default();
                if dir.is_empty() {
                    UserInterface::error("Usage: ous --sort <dir> <arch>");
                    sys_process::exit(1);
                }
                let arch = resolve_repo_arch(&opt_arch)?;
                ous::sort_packages(&dir, &arch)?;
                sys_process::exit(0);
            }
            "--validate" => {
                let index_path = take_value().unwrap_or_default();
                let packages_dir = take_value().unwrap_or_else(|| ".".to_string());
                if index_path.is_empty() {
                    UserInterface::error("Usage: ous --validate <index.json> <packages_dir>");
                    sys_process::exit(1);
                }
                let problems = ous::validate(&index_path, &packages_dir)?;
                sys_process::exit(problems.min(255) as i32);
            }
            "--checksum" => {
                let index_path = take_value().unwrap_or_default();
                let pkg_dir = take_value().unwrap_or_else(|| ".".to_string());
                if index_path.is_empty() {
                    UserInterface::error(
                        "Usage: ous --checksum <index.json> <pkg_dir> [--base-url URL] [--arch ARCH]",
                    );
                    sys_process::exit(1);
                }
                let base_url = opt_base_url
                    .clone()
                    .or_else(|| env::var("CUDANE_REPO_URL").ok())
                    .unwrap_or_else(|| "https://raw.codeberg.org/Cudane/Repository".to_string());
                let arch = resolve_repo_arch(&opt_arch)?;
                ous::checksum_index(&index_path, &pkg_dir, &base_url, &arch)?;
                sys_process::exit(0);
            }
            "--source" => {
                let index_path = take_value().unwrap_or_default();
                if index_path.is_empty() {
                    UserInterface::error(
                        "Usage: ous --source <index.json> [--base-url URL] [--arch ARCH]",
                    );
                    sys_process::exit(1);
                }
                let base_url = opt_base_url
                    .clone()
                    .or_else(|| env::var("CUDANE_REPO_URL").ok())
                    .unwrap_or_else(|| "https://raw.codeberg.org/Cudane/Repository".to_string());
                let arch = resolve_repo_arch(&opt_arch)?;
                ous::rewrite_source(&index_path, &base_url, &arch)?;
                sys_process::exit(0);
            }
            "-u" | "--upload" => {
                if let Some(val) = take_value() {
                    unsafe { env::set_var("OUS_UPLOAD_URL", val) };
                }
            }
            "--token" => {
                if let Some(val) = take_value() {
                    unsafe { env::set_var("OUS_UPLOAD_TOKEN", val) };
                }
            }
            "--upload-index" => unsafe { env::set_var("OUS_UPLOAD_INDEX", "1") },
            "-g" | "--sign" => {
                let index_path = take_value().unwrap_or_default();
                let packages_dir = take_value().unwrap_or_else(|| ".".to_string());
                if index_path.is_empty() {
                    UserInterface::error(
                        "Usage: ous --sign <index.json> <packages_dir> [--key KEYID]",
                    );
                    sys_process::exit(1);
                }
                let key_id = match opt_key.clone() {
                    Some(k) if !k.is_empty() => k,
                    _ => env::var("GPG_KEY_ID").unwrap_or_else(|_| {
                        UserInterface::error("--key <KEYID> or GPG_KEY_ID env var required");
                        sys_process::exit(1);
                    }),
                };
                ous::sign_packages(&index_path, &packages_dir, &key_id)?;
                sys_process::exit(0);
            }
            "-n" | "--no-auto" => unsafe { env::set_var("OUS_NO_AUTO", "1") },
            "-f" | "--force" => unsafe { env::set_var("OUS_FORCE", "1") },
            "-c" | "--clean" => unsafe { env::set_var("OUS_CLEAN", "1") },
            "-s" | "--strict" => unsafe { env::set_var("OUS_STRICT", "1") },
            "-q" | "--quiet" => unsafe { env::set_var("OUS_QUIET", "1") },
            "-d" | "--debug" => unsafe { env::set_var("OUS_DEBUG", "1") },
            "-y" | "--yes" => unsafe { env::set_var("OUS_ASSUME_YES", "1") },
            "-l" | "--parallel" => unsafe { env::set_var("OUS_PARALLEL", "1") },
            "-m" | "--manifest" => {
                if let Some(val) = take_value() {
                    manifest_path = val;
                }
            }
            "-o" | "--output" => {
                if let Some(val) = take_value() {
                    output_dir = val;
                }
            }
            "-j" | "--jobs" => {
                if let Some(val) = take_value() {
                    unsafe { env::set_var("OUS_JOBS", val) };
                }
            }
            "-z" | "--zstd-level" => {
                if let Some(val) = take_value() {
                    match val.parse::<u32>() {
                        Ok(level) => unsafe {
                            env::set_var("OUS_ZSTD_LEVEL", level.clamp(1, 22).to_string())
                        },
                        Err(_) => {
                            UserInterface::error(&format!(
                                "Invalid zstd level '{}': expected an integer between 1 and 22",
                                val
                            ));
                            sys_process::exit(1);
                        }
                    }
                }
            }
            "-p" | "--project" => {
                if let Some(val) = take_value() {
                    unsafe { env::set_var("OUS_PROJECT_WORKSPACE", val) };
                }
            }
            "-t" | "--target" => {
                if let Some(val) = take_value() {
                    unsafe { env::set_var("OUS_TARGET", val) };
                }
            }
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
        UserInterface::info(
            "Usage: ous [OPTIONS] <MANIFEST> <OUTPUT_DIR>\nTry 'ous --help' for more information.",
        );
        sys_process::exit(1);
    }

    let manifest_content = fs::read_to_string(&manifest_path).map_err(|e| {
        UserInterface::error(&format!("Failed to read manifest {}: {}", manifest_path, e));
        anyhow!("Failed to read manifest {}: {}", manifest_path, e)
    })?;

    let mut manifest: Manifest = serde_json::from_str(&manifest_content).map_err(|e| {
        UserInterface::error(&format!("Invalid JSON in manifest: {}", e));
        anyhow!("Invalid JSON in manifest: {}", e)
    })?;

    fs::create_dir_all(&output_dir).map_err(|e| {
        UserInterface::error(&format!(
            "Failed to create output directory {}: {}",
            output_dir, e
        ));
        anyhow!("Failed to create output directory {}: {}", output_dir, e)
    })?;

    // Reject ambiguous manifests up front: two entries sharing (name, arch)
    // would race for the same workspace/output paths in parallel mode.
    {
        let mut seen = std::collections::HashSet::new();
        for pkg in &manifest.packages {
            if !seen.insert((pkg.name.clone(), pkg.arch.clone())) {
                UserInterface::error(&format!(
                    "Duplicate package entry '{}' for architecture '{}' in manifest",
                    pkg.name, pkg.arch
                ));
                sys_process::exit(1);
            }
        }
    }

    let parallel = env::var("OUS_PARALLEL").is_ok();
    let jobs: usize = env::var("OUS_JOBS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    if parallel && manifest.packages.len() > 1 {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::mpsc;
        use std::thread;

        struct ActiveGuard {
            active: Arc<AtomicUsize>,
        }
        impl Drop for ActiveGuard {
            fn drop(&mut self) {
                self.active.fetch_sub(1, Ordering::SeqCst);
            }
        }

        let packages = Arc::new(std::mem::take(&mut manifest.packages));
        let (tx, rx) = mpsc::channel::<String>();
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let next_idx = Arc::new(std::sync::Mutex::new(0usize));
        let quiet = env::var("OUS_QUIET").is_ok();
        let max_concurrent = jobs.max(1);

        let mut handles = Vec::new();
        for _ in 0..max_concurrent.min(packages.len()) {
            let tx = tx.clone();
            let stop = Arc::clone(&stop);
            let active = Arc::clone(&active);
            let next_idx = Arc::clone(&next_idx);
            let packages = Arc::clone(&packages);
            let out = output_dir.clone();
            handles.push(thread::spawn(move || {
                loop {
                    // Cooperative cancellation: stop pulling new work as soon as
                    // any sibling reports failure.
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    while active.load(Ordering::SeqCst) >= max_concurrent
                        && !stop.load(Ordering::SeqCst)
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let claimed = {
                        let mut guard = next_idx.lock().expect("index mutex poisoned");
                        if *guard < packages.len() {
                            let i = *guard;
                            *guard += 1;
                            Some(i)
                        } else {
                            None
                        }
                    };
                    let Some(i) = claimed else { break };
                    let _guard = ActiveGuard {
                        active: Arc::clone(&active),
                    };
                    active.fetch_add(1, Ordering::SeqCst);
                    match process(&packages[i], &out) {
                        Ok(p) => {
                            let _ = tx.send(if quiet {
                                String::new()
                            } else {
                                format!("OK: {}", p)
                            });
                        }
                        Err(e) => {
                            stop.store(true, Ordering::SeqCst);
                            let _ = tx.send(format!("Abort: {}", e));
                        }
                    }
                }
            }));
        }
        drop(tx);

        // Drain every remaining result before exiting: in-flight packages are
        // always allowed to finish so the workspace never ends half-written.
        let mut aborted = false;
        for msg in rx.iter() {
            if msg.starts_with("Abort:") {
                aborted = true;
                UserInterface::error(&msg);
            } else if !quiet && !msg.is_empty() {
                UserInterface::success(&msg);
            }
        }
        for h in handles {
            let _ = h.join();
        }
        if aborted {
            sys_process::exit(1);
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
