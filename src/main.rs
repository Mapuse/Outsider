pub mod utils;

use crate::utils::ui::UserInterface;

use anyhow::{anyhow, Result};
use os::{process, Manifest, PackageMetadata};
use std::env;
use std::fs;
use std::path::Path;
use std::process as sys_process;

fn main() -> Result<()> {
    let mut args = env::args().skip(1).peekable();
    let mut manifest_path = String::new();
    let mut output_dir = String::new();

    if args.len() == 0 {
        utils::UserInterface::info("Usage: os [OPTIONS] <MANIFEST> <OUTPUT_DIR>\nTry 'os --help' for more information.");
        sys_process::exit(1);
    }

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("Render Line (Outsider) Build Engine\n");
                utils::UserInterface::info("USAGE:");
                println!("  os [OPTIONS] <MANIFEST> <OUTPUT_DIR>\n");
                utils::UserInterface::info("OPTIONS:");
                println!("  -a, --archive <SRC> <OUT>  Manually archive a directory using tar.zstd (.xcs)");
                println!("  -x, --extract <PKG> <DEST> Extract standalone package(s) into target rootfs");
                println!("  -g, --hash-type <TYPE>     Set the checksum algorithm for package metadata (default: sha256)");
                println!("  -w, --write <SRC> <DEST>   Generate metadata.json for directory without archiving");
                println!("  -i, --inspect <PKG>        Inspect package specifications, size, and metadata");
                println!("  -m, --manifest <FILE>      Path to manifest.json");
                println!("  -o, --output <DIR>         Path to output directory");
                println!("  -n, --no-auto              Disable automatic build/install behaviors");
                println!("  -f, --force                Overwrite existing .xcs packages");
                println!("  -c, --clean                Clean workspace before building");
                println!("  -s, --strict               Fail immediately on dependency mapping errors");
                println!("  -q, --quiet                Suppress standard output messages");
                println!("  -d, --debug                Enable verbose debug logging");
                println!("  -y, --yes                  Assume 'yes' to all prompts");
                println!("  -k, --keep-src             Do not delete source directory after build");
                println!("  -l, --parallel             Enable parallel package processing");
                println!("  -j, --jobs <NUM>           Set number of parallel make jobs");
                println!("  -z, --zstd-level <NUM>     Set zstd compression level for tar");
                println!("  -p, --project <DIR>        Define custom project/workspace directory");
                println!("  -t, --target <ARCH>        Define target architecture");
                println!("  -v, --version              Print version information");
                sys_process::exit(0);}
            "-v" | "--version" => {
                println!("Outsider 0.5.0");
                sys_process::exit(0);
            }
            "-a" | "--archive" => {
                let staging_dir = args.next().into_iter().next().unwrap_or_default();
                let output_package = args.next().into_iter().next().unwrap_or_default();
                if staging_dir.is_empty() || output_package.is_empty() {
                    UserInterface::error(&format!("Usage: os -a <staging_dir> <output_package.xcs>"));
                    sys_process::exit(1);
                }
                let status = sys_process::Command::new("sh")
                    .args(["-c", &format!("tar -c -C {} . | zstd -3 > {}", staging_dir, output_package)])
                    .status()?;
                if !status.success() { UserInterface::error(&format!("Manual archive compression failed")); }
                UserInterface::info(&format!("Successfully archived {} to {}", staging_dir, output_package));
                sys_process::exit(0);
            }
            "-x" | "--extract" => {
                let input_package = args.next().into_iter().next().unwrap_or_default();
                let root = args.next().into_iter().next().unwrap_or_default();
                if input_package.is_empty() || root.is_empty() {
                    UserInterface::error(&format!("Usage: os -x <package.xcs|directory> <root>"));
                    sys_process::exit(1);
                }
                fs::create_dir_all(&root)?;
                let path_obj = Path::new(&input_package);
                let mut packages = Vec::new();
                if path_obj.is_dir() {
                    if let Ok(entries) = fs::read_dir(path_obj) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file() && p.extension().map_or(false, |ext| ext == "xcs") {
                                packages.push(p);
                            }
                        }
                    }
                } else {
                    packages.push(path_obj.to_path_buf());
                }
                for f in packages {
                    println!("Unpacking package: {}", f.file_name().unwrap().to_string_lossy());
                    let _ = sys_process::Command::new("sh")
                        .args(["-c", &format!("tar --zstd -xf '{0}' -C '{1}' 2>/dev/null || zstd -dc '{0}' | tar -xf - -C '{1}' 2>/dev/null || true", f.to_string_lossy(), root)])
                        .status();
                }
                sys_process::exit(0);
            }
            "-g" | "--hash-type" => {
                if let Some(val) = args.next() {
                    unsafe { env::set_var("OUS_HASH_TYPE", val) };
                }
            }
            "-w" | "--write" => {
                let src_dir = args.next().into_iter().next().unwrap_or_default();
                let dest_dir = args.next().into_iter().next().unwrap_or_default();
                if src_dir.is_empty() || dest_dir.is_empty() {
                    UserInterface::error(&format!("Usage: os -w <src_dir> <dest_dir>"));
                    sys_process::exit(1);
                }
                let mut pkg_name = "custom-package".to_string();
                if let Some(name) = Path::new(&src_dir).file_name() {
                    pkg_name = name.to_string_lossy().into_owned();
                }
                let sum = os::hash(&dest_dir).unwrap_or_default();
                let repo_root = env::current_dir()?;
                let mock_pkg = os::Package {
                    name: pkg_name.clone(),
                    version: "manual".into(),
                    source: "manual".into(),
                    build_type: "manual".into(),
                    build_cmd: "".into(),
                    install_cmd: "".into(),
                    links: None,
                };
                
                let metadata = match os::mtd(&mock_pkg, &dest_dir, &sum, &src_dir, "", &repo_root) {
                    Ok(meta) => meta,
                    Err(_) => PackageMetadata {
                        pkg_name,
                        version: "manual".into(),
                        source: "manual".into(),
                        license: "Unknown".into(),
                        checksum: sum[0].clone(),
                        dependencies: Vec::new(),
                        files: Vec::new(),
                        provides: None,
                        conflicts: None,
                    }
                };

                os::write(&metadata, &dest_dir)?;
                UserInterface::success(&format!("Successfully generated metadata.json inside {}", dest_dir));
                sys_process::exit(0);
            }
            "-i" | "--inspect" => {
                let input_package = args.next().into_iter().next().unwrap_or_default();
                if input_package.is_empty() {
                    UserInterface::error(&format!("Usage: os -i <path/to/package.xcs>"));

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
                
                let mut embedded: Vec<os::Checksum> = Vec::new();
                if let Ok(decoder) = zstd::stream::Decoder::new(file) {
                    let mut archive = tar::Archive::new(decoder);
                    if let Ok(entries) = archive.entries() {
                        for entry in entries.flatten() {
                            if let Ok(path) = entry.path() {
                                if path.file_name().map_or(false, |n| n == "metadata.json") {
                                    if let Ok(meta_struct) = serde_json::from_reader::<_, os::PackageMetadata>(entry) {
                                        embedded = vec![meta_struct.checksum];
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                let _file = fs::File::open(path_obj)?;
                let current_checksums = os::hash(path_obj.to_str().unwrap_or_default())
                    .unwrap_or_default();

                if embedded.is_empty() {
                    UserInterface::warning(&format!("Status           : \x1b[33m[WARNING]\x1b[0m Embedded metadata.json not found inside archive."));
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
            "-z" | "--zstd" => {
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
        UserInterface::info("Usage: os [OPTIONS] <MANIFEST> <OUTPUT_DIR>\nTry 'os --help' for more information.");
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
    
    Ok(())
}