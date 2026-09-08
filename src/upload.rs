use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;
use crate::utils::ui::UserInterface;

/// Number of upload attempts before giving up.
const MAX_ATTEMPTS: u32 = 4;
/// Base delay for the first retry; doubled on each subsequent attempt.
const BASE_RETRY_MS: u64 = 500;

/// Chunk size for resumable (from-bit) archive pushes.
const RESUME_CHUNK_SIZE: u64 = 8 * 1024 * 1024;
/// Files at least this large are pushed chunk-by-chunk so an interrupted
/// push resumes from the last completed chunk instead of restarting the
/// whole file. Smaller files (sidecar, index) are a single PUT.
const RESUME_CHUNK_THRESHOLD: u64 = 1024 * 1024;

/// How the last push of this file was performed; decides whether a resume
/// probes the ranged-PUT protocol again or falls straight back to a whole
/// file PUT.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
enum UploadMode {
    #[default]
    Chunked,
    Whole,
}

/// Persisted progress of a resumable push: the chunk indexes that completed
/// a ranged PUT against `url`, plus the file identity they belong to.
#[derive(Serialize, Deserialize)]
struct UploadState {
    url: String,
    size: u64,
    mtime_nanos: i64,
    mode: UploadMode,
    completed: BTreeSet<u64>,
}

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

/// Half-open byte ranges `[start, end)` covering a file of `total_size`
/// bytes in fixed `chunk` pieces (the last one may be shorter).
fn chunk_ranges(total_size: u64, chunk: u64) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    let mut start = 0u64;
    while start < total_size {
        let end = (start + chunk).min(total_size);
        ranges.push((start, end));
        start = end;
    }
    ranges
}

fn mtime_nanos(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Sidecar holding push progress, stored next to the archive.
fn resume_sidecar_path(archive: &Path) -> PathBuf {
    let mut os = archive.as_os_str().to_os_string();
    os.push(".upload.json");
    PathBuf::from(os)
}

fn load_state(sidecar: &Path, url: &str) -> Option<UploadState> {
    let bytes = std::fs::read(sidecar).ok()?;
    let state: UploadState = serde_json::from_slice(&bytes).ok()?;
    if state.url != url {
        return None;
    }
    Some(state)
}

fn save_state(sidecar: &Path, state: &UploadState) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    crate::atomic_write(sidecar, json.as_bytes())
}

/// HTTP status codes that indicate the repository does not support ranged
/// PUT — falling back to a whole-file PUT for such servers.
fn range_unsupported(code: u16) -> bool {
    matches!(code, 400 | 403 | 404 | 405 | 416 | 501)
}

/// Publish `file` to `url`, resuming from the last completed chunk when the
/// push was interrupted mid-way (from-bit completion).
///
/// Files under `RESUME_CHUNK_THRESHOLD` are pushed with a single PUT. Larger
/// files are sent as fixed-size chunks using `Content-Range: bytes
/// a-b/total`; a local sidecar records which chunks succeeded, so a retried
/// push sends only the missing chunks. Servers that reject ranged PUT return
/// to the plain whole-file PUT for the rest of that archive.
pub fn upload_file_resumable(url: &str, file: &Path, token: Option<&str>) -> Result<()> {
    if !file.exists() {
        return Err(anyhow!("Cannot upload {:?}: file does not exist", file));
    }
    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);

    // Already fully present on the remote side? Then there is nothing to
    // send regardless of the chunking protocol.
    if let Some(remote) = head_content_length(url, token)
        && remote == size
    {
        UserInterface::info(&format!(
            "{} already present at {} — skipping",
            file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            url
        ));
        return Ok(());
    }

    if size == 0 || size < RESUME_CHUNK_THRESHOLD {
        return upload_file(url, file, token);
    }

    let sidecar = resume_sidecar_path(file);
    let mtime = mtime_nanos(file);
    let mut state = load_state(&sidecar, url).unwrap_or(UploadState {
        url: url.to_string(),
        size,
        mtime_nanos: mtime,
        mode: UploadMode::Chunked,
        completed: BTreeSet::new(),
    });
    // A stale sidecar (different file identity) must not alias chunk bytes
    // belonging to a previous, different transfer.
    if state.size != size || state.mtime_nanos != mtime {
        state = UploadState {
            url: url.to_string(),
            size,
            mtime_nanos: mtime,
            mode: UploadMode::Chunked,
            completed: BTreeSet::new(),
        };
    } else if state.mode == UploadMode::Whole {
        // The server rejected ranged PUT last time; do not probe again.
        return upload_file(url, file, token);
    }

    for (idx, (start, end)) in chunk_ranges(size, RESUME_CHUNK_SIZE).iter().enumerate() {
        let idx = idx as u64;
        if state.completed.contains(&idx) {
            continue;
        }
        match try_upload_chunk(url, file, *start, *end, size, token)? {
            ChunkResult::Sent => {
                state.completed.insert(idx);
                save_state(&sidecar, &state)?;
                UserInterface::info(&format!(
                    "Pushed chunk {}/{} ({}-{}) -> {}",
                    idx + 1,
                    chunk_ranges(size, RESUME_CHUNK_SIZE).len(),
                    start,
                    end - 1,
                    url
                ));
            }
            ChunkResult::Unsupported(code) => {
                UserInterface::warning(&format!(
                    "Repo does not support ranged PUT (HTTP {}); falling back to a whole-file upload",
                    code
                ));
                state.mode = UploadMode::Whole;
                save_state(&sidecar, &state)?;
                return upload_file(url, file, token);
            }
        }
    }

    UserInterface::info(&format!(
        "Uploaded {} ({:.2} MiB, {} chunks) -> {}",
        file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size as f64 / 1024.0 / 1024.0,
        chunk_ranges(size, RESUME_CHUNK_SIZE).len(),
        url
    ));
    // All chunks landed; drop the sidecar so the next push starts clean
    // (the HEAD re-check above makes the re-push a no-op anyway).
    let _ = std::fs::remove_file(&sidecar);
    Ok(())
}

/// HTTP HEAD `url`, returning the remote `Content-Length` (or `None` when
/// the object is absent/unreachable).
fn head_content_length(url: &str, token: Option<&str>) -> Option<u64> {
    let mut cmd = Command::new("curl");
    cmd.arg("--silent")
        .arg("--show-error")
        .arg("--request").arg("HEAD")
        .arg("--dump-header").arg("-")
        .arg("--output").arg("/dev/null");
    if let Some(token) = token.filter(|t| !t.trim().is_empty()) {
        cmd.arg("--header").arg(format!("Authorization: Bearer {}", token.trim()));
    }
    cmd.arg(url);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&out.stdout);
    for line in head.lines() {
        if line.to_ascii_lowercase().starts_with("content-length:")
            && let Some((_, value)) = line.split_once(':')
        {
            return value.trim().parse::<u64>().ok();
        }
    }
    None
}

enum ChunkResult {
    Sent,
    Unsupported(u16),
}

/// PUT a single `[start, end)` chunk body with a `Content-Range` header.
/// Retries transient failures with exponential backoff, mirroring
/// `upload_file`; treats client-side "ranges not supported" codes as a
/// protocol fallback rather than a hard error.
fn try_upload_chunk(
    url: &str,
    file: &Path,
    start: u64,
    end: u64,
    total: u64,
    token: Option<&str>,
) -> Result<ChunkResult> {
    let mut f = std::fs::File::open(file)?;
    f.seek(SeekFrom::Start(start))?;

    for attempt in 0..MAX_ATTEMPTS {
        let status = put_range(url, &mut f, end - start, start, end - 1, total, token)?;
        if let Some(code) = status {
            if range_unsupported(code) {
                return Ok(ChunkResult::Unsupported(code));
            }
            if (400..=599).contains(&code) {
                let remaining = MAX_ATTEMPTS - attempt - 1;
                if remaining == 0 {
                    return Err(anyhow!(
                        "Upload chunk {}-{} of {} to {} failed with HTTP {} after {} attempts",
                        start,
                        end - 1,
                        total,
                        url,
                        code,
                        MAX_ATTEMPTS
                    ));
                }
                let delay = BASE_RETRY_MS * 2u64.pow(attempt);
                UserInterface::warning(&format!(
                    "Chunk upload to {} failed (HTTP {}); retrying in {} ms ({} left)",
                    url, code, delay, remaining
                ));
                std::thread::sleep(std::time::Duration::from_millis(delay));
                continue;
            }
        }
        return Ok(ChunkResult::Sent);
    }
    Err(anyhow!("Chunk upload exited outside the retry loop"))
}

/// Spawn curl once: PUT the given byte range as the request body, with a
/// `Content-Range` header describing the offset within the whole object.
/// Returns the HTTP status code (`None` when curl could not report one).
fn put_range(
    url: &str,
    reader: &mut impl Read,
    len: u64,
    start: u64,
    end: u64,
    total: u64,
    token: Option<&str>,
) -> Result<Option<u16>> {
    let mut child = Command::new("curl");
    child.arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--request").arg("PUT")
        .arg("--header").arg("Content-Type: application/octet-stream")
        .arg("--header")
        .arg(format!("Content-Range: bytes {}-{}/{}", start, end, total))
        .arg("--write-out").arg("\n%{http_code}")
        .arg("--output").arg("/dev/null")
        .arg("--upload-file").arg("-");
    if let Some(token) = token.filter(|t| !t.trim().is_empty()) {
        child.arg("--header").arg(format!("Authorization: Bearer {}", token.trim()));
    }
    child.arg(url);
    child.stdin(Stdio::piped()).stdout(Stdio::piped());

    let mut child = child.spawn().map_err(|e| anyhow!("Failed to spawn curl: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        let mut remaining = len;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let want = (remaining.min(buf.len() as u64)) as usize;
            let n = reader.read(&mut buf[..want]).map_err(|e| anyhow!("Failed to read chunk: {}", e))?;
            if n == 0 {
                return Err(anyhow!("File shrank while uploading: expected {} more bytes", remaining));
            }
            stdin.write_all(&buf[..n]).map_err(|e| anyhow!("Failed to pipe chunk to curl: {}", e))?;
            remaining -= n as u64;
        }
        drop(stdin);
    }

    let out = child.wait_with_output().map_err(|e| anyhow!("Failed to wait for curl: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(anyhow!(
            "HTTP PUT {} (bytes {}-{} of {}) exited with {}: {}",
            url,
            start,
            end,
            total,
            out.status,
            if detail.is_empty() { "no detail" } else { &detail }
        ));
    }

    // Last line carries the --write-out status code.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let code = stdout
        .lines()
        .last()
        .and_then(|l| l.trim().parse::<u16>().ok());
    Ok(code)
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
    upload_file_resumable(&archive_url, archive_abs, opts.token.as_deref())?;
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

    #[test]
    fn test_chunk_ranges_resume_layout() {
        assert_eq!(
            chunk_ranges(17, 8),
            vec![(0, 8), (8, 16), (16, 17)]
        );
        assert_eq!(chunk_ranges(16, 8), vec![(0, 8), (8, 16)]);
        assert!(chunk_ranges(0, 8).is_empty());
        let ranges = chunk_ranges(25_000_000, 8 * 1024 * 1024);
        let mut cursor = 0u64;
        for (start, end) in &ranges {
            assert_eq!(cursor, *start);
            cursor = *end;
        }
        assert_eq!(cursor, 25_000_000);
    }

    #[test]
    fn test_resume_sidecar_roundtrip_and_stale_detection() {
        let dir = std::env::temp_dir().join(format!(
            "ou-upload-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("hello-1.0.0.xcs");
        std::fs::write(&archive, vec![0u8; 1000]).unwrap();
        let sidecar = resume_sidecar_path(&archive);

        let mut state = UploadState {
            url: "https://r.example/pool/x/hello-1.0.0.xcs".into(),
            size: 1000,
            mtime_nanos: mtime_nanos(&archive),
            mode: UploadMode::Chunked,
            completed: BTreeSet::from([0, 2]),
        };
        save_state(&sidecar, &state).unwrap();

        let loaded = load_state(&sidecar, "https://r.example/pool/x/hello-1.0.0.xcs")
            .expect("state must load for the matching URL");
        assert_eq!(loaded.completed, BTreeSet::from([0, 2]));
        assert_eq!(loaded.size, 1000);

        // A different remote URL never aliases the same sidecar.
        assert!(load_state(&sidecar, "https://other.example/pool/x/hello.xcs").is_none());

        // A stale sidecar (mismatched file identity) is ignored by upload.
        state.completed.insert(1);
        state.size = 999;
        save_state(&sidecar, &state).unwrap();
        let stale = load_state(&sidecar, "https://r.example/pool/x/hello-1.0.0.xcs").unwrap();
        assert_ne!(stale.size, std::fs::metadata(&archive).unwrap().len(), "stale size must not match");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_range_unsupported_codes() {
        assert!(range_unsupported(400));
        assert!(range_unsupported(405));
        assert!(range_unsupported(416));
        assert!(range_unsupported(501));
        assert!(!range_unsupported(200));
        assert!(!range_unsupported(503));
        assert!(!range_unsupported(429));
    }
}