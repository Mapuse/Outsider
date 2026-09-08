use anyhow::{Result, anyhow};
use std::path::Path;
use std::process::Command;
use crate::utils::ui::UserInterface;

/// Number of upload attempts before giving up.
const MAX_ATTEMPTS: u32 = 4;
/// Base delay for the first retry; doubled on each subsequent attempt.
const BASE_RETRY_MS: u64 = 500;

/// Join a repository base URL with a relative pool path, tolerating a
/// trailing and/or leading slash on either side.
pub fn join_url(base_url: &str, rel: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    format!("{}/{}", base, rel)
}

/// Resolve the repository-relative layout for a published package, following
/// the pool layout mcx consumes: `pool/<arch>/<name>/<name>-<ver>.xcs` plus
/// its `.sha256` integrity sidecar.
pub fn pool_paths(pkg_name: &str, pkg_version: &str, arch: &str) -> (String, String) {
    let arch_dir = if arch.is_empty() || arch == "native" {
        "native".to_string()
    } else {
        arch.to_string()
    };
    let file = format!("{}-{}.xcs", pkg_name, pkg_version);
    let dir = format!("pool/{}/{}/", arch_dir, pkg_name);
    let archive = format!("{}{}", dir, file);
    let sidecar = format!("{}.sha256", archive);
    (archive, sidecar)
}

/// Index filename for an architecture (equal to the one clients fetch as
/// `index.<arch>.json`).
pub fn index_path(arch: &str) -> String {
    let arch = if arch.is_empty() || arch == "native" {
        "native"
    } else {
        arch
    };
    format!("index.{}.json", arch)
}

/// Upload a single file with an HTTP PUT. Retries with exponential backoff
/// for transient failures; succeeds only when the server returns 2xx.
pub fn upload_file(url: &str, file: &Path, token: Option<&str>) -> Result<()> {
    if !file.exists() {
        return Err(anyhow!("Cannot upload {:?}: file does not exist", file));
    }
    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);

    for attempt in 0..MAX_ATTEMPTS {
        match try_upload(url, file, token) {
            Ok(()) => {
                UserInterface::info(&format!(
                    "Uploaded {} ({:.2} MiB) -> {}",
                    file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    size as f64 / 1024.0 / 1024.0,
                    url
                ));
                return Ok(());
            }
            Err(e) => {
                let remaining = MAX_ATTEMPTS - attempt - 1;
                if remaining == 0 {
                    return Err(anyhow!(
                        "Failed to upload {} to {} after {} attempts: {}",
                        file.display(),
                        url,
                        MAX_ATTEMPTS,
                        e
                    ));
                }
                let delay = BASE_RETRY_MS * 2u64.pow(attempt);
                UserInterface::warning(&format!(
                    "Upload to {} failed ({}); retrying in {} ms ({} left)",
                    url, e, delay, remaining
                ));
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
        }
    }
    Err(anyhow!("Upload completed outside the retry loop"))
}

fn try_upload(url: &str, file: &Path, token: Option<&str>) -> Result<()> {
    let mut cmd = Command::new("curl");
    cmd.arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--request")
        .arg("PUT")
        .arg("--upload-file")
        .arg(file);
    if let Some(token) = token.filter(|t| !t.trim().is_empty()) {
        cmd.arg("--header")
            .arg(format!("Authorization: Bearer {}", token.trim()));
    }
    cmd.arg(url);

    let out = cmd.output().map_err(|e| anyhow!("Failed to spawn curl: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(anyhow!(
            "HTTP PUT {} exited with {}: {}",
            url,
            out.status,
            if detail.is_empty() { "no detail" } else { &detail }
        ));
    }
    Ok(())
}

/// Settings that control how a finished package is published over HTTP.
#[derive(Debug, Clone)]
pub struct UploadOptions {
    /// Repository base URL (`http://` or `https://`) packages are PUT into.
    pub base_url: String,
    /// Optional bearer token; sent as `Authorization: Bearer <token>`.
    pub token: Option<String>,
    /// Target architecture, used to build the `pool/<arch>/` layout.
    pub arch: String,
    /// Also PUT the updated `index.<arch>.json` to the repository root.
    pub upload_index: bool,
}

/// Publish a built package to a remote repository over HTTP PUT.
///
/// Uploads the `.xcs` archive and its `.sha256` sidecar into the pool layout
/// and, when `upload_opts.upload_index` is set, the provided local
/// `index.<arch>.json` (the one the build pipeline writes into the repository
/// root). Returns the list of URLs that were uploaded.
pub fn upload_package(
    opts: &UploadOptions,
    archive_abs: &Path,
    pkg_name: &str,
    pkg_version: &str,
    index_source: Option<&Path>,
) -> Result<Vec<String>> {
    let base_url = &opts.base_url;
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(anyhow!(
            "Upload base URL must start with http:// or https://: {}",
            base_url
        ));
    }

    let (rel_archive, rel_sidecar) = pool_paths(pkg_name, pkg_version, &opts.arch);
    let sidecar_abs = {
        let mut os = archive_abs.as_os_str().to_os_string();
        os.push(".sha256");
        Path::new(&os).to_path_buf()
    };

    let mut uploaded = Vec::new();

    UserInterface::info(&format!(
        "Publishing {} v{} (arch {}) to {}",
        pkg_name,
        pkg_version,
        arch_display(&opts.arch),
        base_url
    ));

    let archive_url = join_url(base_url, &rel_archive);
    upload_file(&archive_url, archive_abs, opts.token.as_deref())?;
    uploaded.push(archive_url);

    let sidecar_url = join_url(base_url, &rel_sidecar);
    if sidecar_abs.exists() {
        upload_file(&sidecar_url, &sidecar_abs, opts.token.as_deref())?;
        uploaded.push(sidecar_url);
    } else {
        // The archive integrity sidecar is part of the repo contract; refuse
        // to publish an archive without one.
        return Err(anyhow!(
            "Refusing to publish without integrity sidecar {:?}",
            sidecar_abs
        ));
    }

    if opts.upload_index {
        let rel_index = index_path(&opts.arch);
        match index_source {
            Some(src) if src.exists() => {
                let index_url = join_url(base_url, &rel_index);
                upload_file(&index_url, src, opts.token.as_deref())?;
                uploaded.push(index_url);
            }
            Some(src) => UserInterface::warning(&format!(
                "--upload-index requested but {} was not found; index not uploaded",
                src.display()
            )),
            None => UserInterface::warning(
                "--upload-index requested but no index file was supplied; index not uploaded",
            ),
        }
    }

    Ok(uploaded)
}

fn arch_display(arch: &str) -> &str {
    if arch.is_empty() || arch == "native" {
        "native"
    } else {
        arch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_url_slash_tolerance() {
        assert_eq!(join_url("https://repo.example.org", "pool/x/foo-1.xcs"),
                   "https://repo.example.org/pool/x/foo-1.xcs");
        assert_eq!(join_url("https://repo.example.org/", "/pool/x/foo"),
                   "https://repo.example.org/pool/x/foo");
    }

    #[test]
    fn test_pool_paths_native_arch() {
        let (archive, sidecar) = pool_paths("hello", "1.0.0", "native");
        assert_eq!(archive, "pool/native/hello/hello-1.0.0.xcs");
        assert_eq!(sidecar, "pool/native/hello/hello-1.0.0.xcs.sha256");
    }

    #[test]
    fn test_pool_paths_explicit_arch() {
        let (archive, sidecar) = pool_paths("hello", "1.0.0", "aarch64");
        assert_eq!(archive, "pool/aarch64/hello/hello-1.0.0.xcs");
        assert_eq!(sidecar, "pool/aarch64/hello/hello-1.0.0.xcs.sha256");
    }

    #[test]
    fn test_index_path_normalizes_arch() {
        assert_eq!(index_path("native"), "index.native.json");
        assert_eq!(index_path(""), "index.native.json");
        assert_eq!(index_path("x86_64"), "index.x86_64.json");
    }
}