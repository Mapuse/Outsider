#

```shell
 ██████╗ ██╗   ██╗████████╗███████╗██╗██████╗ ███████╗██████╗ 
██╔═══██╗██║   ██║╚══██╔══╝██╔════╝██║██╔══██╗██╔════╝██╔══██╗
██║   ██║██║   ██║   ██║   ███████╗██║██║  ██║█████╗  ██████╔╝
██║   ██║██║   ██║   ██║   ╚════██║██║██║  ██║██╔══╝  ██╔══██╗
╚██████╔╝╚██████╔╝   ██║   ███████║██║██████╔╝███████╗██║  ██║
 ╚═════╝  ╚═════╝    ╚═╝   ╚══════╝╚═╝╚═════╝ ╚══════╝╚═╝  ╚═╝
                                                              
 ██╗ ██████╗ ██╗   ██╗███████╗██╗                             
██╔╝██╔═══██╗██║   ██║██╔════╝╚██╗                            
██║ ██║   ██║██║   ██║███████╗ ██║                            
██║ ██║   ██║██║   ██║╚════██║ ██║                            
╚██╗╚██████╔╝╚██████╔╝███████║██╔╝                            
 ╚═╝ ╚═════╝  ╚═════╝ ╚══════╝╚═╝                             
```

---

`▐▀` `-` `▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▌`

**Outsider (`OUS`)** is an automated source-to-archive build engine designed specifically for the **Cudane** Linux ecosystem (also available for GNU-based distributions). It reads declarative JSON manifests that can contain an unlimited number of package recipes, isolates execution within localized workspaces, builds whatever target you want from source, scans dependencies, packages everything cleanly into `.xcs` binary packages, and automatically writes a unified **`index.json`** for your own repository of packages (with auto-updating support).

`▐▄` `-` `▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▌`

<details><summary id="contents">Contents</summary>

- [Architecture]
- [Manifest]
- [Packages]
- [Symlinks]
- [Metadata]
- [Consolidation]
- [Entry]
- [Core]
- [Binary Reader]
  - [ELF Target Iterator]
  - [Dynamic Symbol Parser]
  - [Dependency Matcher]
  - [Metadata Synchronization & Dependency Signature]
  - [Resolution by ldd]
- [Build / Resume]
- [Starting]
- [Guide]
- [CLI]
- [Format]
- [Indexing]
- [Requirements]
- [Cudane]
- [Manualizing]
- [Lifecycle]
- [License]
- [Credits]

</details>

<details><summary id="arch">Architecture</summary>

`Outsider` is built around a single principle: **declarative input, deterministic output**. You provide a JSON manifest describing what to build and how, and `Outsider` handles the rest — fetching source code, executing builds in isolation, scanning for runtime dependencies, generating metadata, and producing compressed archives.

The codebase is split into two files:

- **`src/main.rs`** — The CLI argument parser and entry point. It parses command-line flags, reads the manifest, and iterates over each package, calling into the library.
- **`src/lib.rs`** — The core engine. Contains all data structures, the fetch/build/install pipeline, metadata generation, dependency injection, license detection, hashing, archiving, component scanning, service detection, sandbox profiling, repository indexing, and the **Binary Reader** dependency scanner.

The engine uses **`anyhow`** for error handling with context propagation, **`serde`** for JSON serialization and deserialization, **`sha2`** for cryptographic hashing, **`chrono`** for timestamp generation, and **`regex`** for license pattern matching. External system tools (`git`, `curl`, `tar`, `ldd`, `file`) are invoked via `std::process::Command` rather than being linked as libraries, keeping the Rust binary lightweight and delegating specialized work to mature system utilities.

</details>

<details><summary id="manifest">Manifest</summary>

The manifest is the single input file that drives the entire build pipeline. It is deserialized into the `Manifest` struct:

```rust
#[[derive(Deserialize, Serialize, Clone)]]
pub struct Manifest {
    pub packages: Vec<Package>,
}
```

> The `Manifest` struct is a thin wrapper around a `Vec<Package>`. There is no limit on the number of packages — a single manifest can define one package or orchestrate an entire operating system bootstrap with hundreds of sequential recipes. The `packages` field is the only top-level key; everything else is expressed within each individual `Package` entry.

</details>

<details><summary id="packages">Packages</summary>

Each element in the `packages` array deserializes into a `Package`:

```rust
#[[derive(Deserialize, Serialize, Clone)]]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: String,
    pub build_type: String,
    pub build_cmd: String,
    pub install_cmd: String,
    pub links: Option<std::collections::HashMap<String, String>>,
    pub arch: String,
}
```

Every field is explained below:

### name (String)

The package identifier. This becomes part of the output filename (`<name>-<version>.xcs`) and is used as the key in the repository index. It should be a simple alphanumeric string, typically lowercase with hyphens (for example, `"hello"`, `"shared-mime-info"`).

### version (String)

The package version string. Combined with `name` to form the unique package identity. Versions are treated as opaque strings — no semantic versioning parsing is performed. The version is embedded in the output filename and in the metadata.

### source (String)

The origin of the source code. This field is processed by the `fetch()` function and supports multiple formats:

- **Remote archive URLs**: `https://example.com/releases/pkg-1.0.0.tar.gz`, `.tar.bz2`, `.tar.xz` — downloaded via `curl` and extracted via `tar`.
- **Git repository URLs**: Any URL ending in `.git` — cloned via `git clone --depth 1` for a shallow, bandwidth-efficient checkout.
- **Local filesystem paths**: `/home/user/projects/my-pkg` or `../relative/path` — copied recursively into the workspace.
- **`file://` URIs**: `file:///absolute/path/to/source` — the `file://` prefix is stripped and the path is treated as a local source.

The `fetch()` function can also check for local existence first (before any protocol-based logic), so local paths always take precedence and avoid network access entirely.

### build_type (String)

A classifier that influences how `Outsider` handles the package when `build_cmd` is empty. Common values include:

- `"rust"` — Triggers automatic `cargo build --release` with Cudane-specific `RUSTFLAGS` when `build_cmd` is empty.
- `"meson"` — Informational; the actual build command must be provided in `build_cmd`.
- `"make"` — Informational; the actual build command must be provided in `build_cmd`.
- `"custom"` — Explicitly signals a custom build process; `build_cmd` and `install_cmd` are expected to be provided.

The `build_type` is also stored in the output metadata for downstream tools to reference.

### build_cmd (String)

The shell command to execute for building the package. The behavior depends on the content:

- **If the trimmed value equals** `"none"`, `"skip"`, or `"nothing"` (case-insensitive): The build step is completely skipped. No command runs, no automatic fallback occurs.
- **If empty** (`""`) **and `OUS_NO_AUTO` is set**: The build step is skipped (returns empty log).
- **If empty** (`""`) **and `build_type` is `"rust"`**: Automatic Rust build is triggered. The engine runs:

```shell
RUSTFLAGS="-C linker=clang -C link-arg=-target \
  -C link-arg=x86_64-pc-linux-musl -C link-arg=--sysroot=/system \
  -C target-feature=+crt-static" \
  cargo build --target x86_64-unknown-linux-musl --release
```

Both stdout and stderr are captured into a `capture.log` file inside the source directory. If the build succeeds, the log content is returned for dependency scanning. If it fails, the error includes the full log output.

- **If empty** (`""`) **and `build_type` is not `"rust"`**: The build step returns an empty string (no-op).
- **If non-empty**: The command is executed via `sh -c` inside the source directory. Both stdout and stderr are captured using `tee` into `capture.log`, which is then read back and deleted. The log content is returned for dependency scanning.

### install_cmd (String)

The shell command to install built artifacts into the staging directory. The behavior depends on the content:

- **If the trimmed value equals** `"none"`, `"skip"`, or `"nothing"` (case-insensitive): The install step is skipped, but any symlinks declared in the `links` map are still created.
- **If empty** (`""`) **and `OUS_NO_AUTO` is set**: The install step is skipped (symlinks still processed).
- **If empty** (`""`) **and `build_type` is `"rust"`**: Automatic Rust install is triggered. The engine copies all files from `target/release/` inside the source directory into the package staging directory. This provides a sensible default for Rust projects where the compiled binaries are placed in `target/release/`.
- **If empty** (`""`) **and `build_type` is not `"rust"`**: Falls through to execute the empty string as a command (which would do nothing), then processes symlinks.
- **If non-empty**: The command is executed via `sh -c` with the `CUDANE_DEST` environment variable set to the package staging directory path. The command runs with the source directory as its working directory. After the command completes, any symlinks in the `links` map are created.

### arch (String)

The target architecture for the package. Supports multi-arch builds — common values are `"amd64"`, `"arm64"`, or `"native"` (which means build for the host architecture). When omitted from the manifest, it defaults to `"native"` for backward compatibility. The architecture is propagated into the package metadata and can be used by downstream tools to select the correct package variant for a given target platform.

### links (Option<HashMap<String, String>>)

An optional map of symbolic links to create inside the package staging directory after installation. The map keys are the **target** paths (what the symlink points to) and the values are the **link** paths (where the symlink is placed). For example:

```json
"links": {
  "system/lib/libexample.so.1": "system/lib/libexample.so"
}
```

This creates a symlink at `pkg_root/system/lib/libexample.so` that points to `system/lib/libexample.so.1`. The `symlink()` function handles the path logic: it strips leading slashes from the link path to keep it relative to the staging root, creates parent directories as needed, removes any existing file at the link location, and then creates the symlink using `std::osunix::fs::symlink`.

</details>

<details><summary id="symlinks">Symlinks</summary>

Although the `Package` struct uses a raw `HashMap<String, String>` for links, there is also a dedicated `Symlink` struct in the codebase:

```rust
#[[derive(Deserialize, Serialize, Clone)]]
pub struct Symlink {
    pub target: String,
    pub link: String,
}
```

This struct is available for serialization and deserialization but is not currently used by the main pipeline — the `links` field in `Package` uses the HashMap directly. And the `Symlink` struct exists as a potential future expansion point for more structured symlink definitions.

</details>

<details><summary id=metadatah">Metadata</summary>

Every built package generates a `metadata.json` file embedded inside the archive. The structure is:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PackageMetadata {
    pub pkg_name: String,
    pub version: String,
    pub license: String,
    pub source: String,
    pub arch: String,
    pub checksum: Checksum,
    pub dependencies: Vec<Dependency>,
    pub files: Vec<PathBuf>,
    pub provides: Option<Vec<String>>,
    pub conflicts: Option<Vec<String>>,
}
```

Each field is populated as follows:

- **`pkg_name`**: Mirrored directly from the manifest's `name` field.
- **`version`**: Mirrored directly from the manifest's `version` field.
- **`source`**: Mirrored directly from the manifest's `source` field.
- **`arch`**: Mirrored directly from the manifest's `arch` field. Defaults to `""` when absent (backward compat with older metadata).
- **`license`**: Determined by the `license()` function, which scans the source directory for license files and extracts the license name using regex pattern matching.
- **`build_type`**: Mirrored directly from the manifest's `build_type` field.
- **`build_date`**: An ISO 8601 UTC timestamp generated at runtime via `chrono::Utc::now().to_rfc3339()`, recording exactly when the metadata was created.
- **`checksum`**: A SHA-256 hex digest of the entire package staging directory, computed by `hash()` which pipes the directory through `tar -cf -` and hashes the resulting byte stream.
- **`provides`**: Using `libdep` and `normalize` to list the libraries that the package provides.
- **`conflicts`**: Using `scan` to scan for any conflicting links and list it.

### Dependency

Each element in the `dependencies` array deserializes into:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub dep_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraries: Option<Vec<String>>,
}
```

- **`name`**: The name of the depended-on package (or a raw library name if the library could not be mapped to any known package).
- **`dep_type`**: A human-readable description of how the dependency was discovered. Values include:
  - `"Build"` — discovered from build log parsing (e.g., `pkg-config --libs foo`, `checking for foo...`, `dependency foo found`)
  - `"Library (libfoo.so.1)"` — discovered by scanning ELF binaries in the staging directory and resolving the library to a known package
  - `"Library"` — a raw library that could not be mapped to any known package in the workspace
  - `"Transitive"` — discovered via transitive dependency resolution against the repository index
- **`libraries`** (optional): If the dependency on a single package arises from **2 or more distinct libraries** (all provided by the same package), this field lists those specific library names. This is the **Consolidation** feature — see the [Consolidation] section.

The `PartialEq` derive is used by `index()` to compare metadata entries and avoid unnecessary updates when the content has not changed.

</details>

<details><summary id="lib-consolidation">Consolidation</summary>

When a package depends on multiple shared libraries that are all provided by a single external package, `Outsider` consolidates them into a single dependency entry with a `libraries` field listing the specific libraries that triggered the dependency.

### Motivation

Consider a scenario where package `my-app` links against `libssl.so.3` and `libcrypto.so.3`, both provided by the `openssl` package. Without consolidation, the dependency list would contain a single entry like:

```json
{
  "name": "openssl",
  "dep_type": "Library (libssl.so.3) & Library (libcrypto.so.3)"
}
```

With consolidation, when 2 or more libraries map to the same package, the entry becomes:

```json
{
  "name": "openssl",
  "dep_type": "Library (libssl.so.3) & Library (libcrypto.so.3)",
  "libraries": ["libcrypto.so.3", "libssl.so.3"]
}
```

The `libraries` field is **only present** when there are at least 2 distinct libraries that resolve to the same package. If only one library maps to a package, no `libraries` field is emitted — keeping the JSON clean and backward compatible.

### Mechanism

The consolidation happens in the `scan()` function in `src/lib.rs`. During dependency resolution:

1. **Library enumeration**: `libdep()` scans all ELF binaries and `.so` files in the staging directory to build a set of needed library filenames.
2. **Package resolution**: Each library name is normalized (e.g., `libfoo.so.1` → `["libfoo.so.1", "libfoo.so", "foo.so.1"]`) and looked up in the workspace library index (`mltp()`) to find which packages provide a matching library.
3. **Consolidation**: `pkg_libs` — a `HashMap<String, Vec<String>>` — tracks every library→package mapping. When converting the dependency map to the final `Vec<Dependency>`, any package with 2 or more entries in `pkg_libs` gets its `libraries` field populated with the sorted, deduplicated list of depended-upon libraries.

Only the **actually depended-upon** libraries are listed in the `libraries` field — not every library that the providing package ships. This gives a precise picture of why the dependency exists.

### Example

Given a package `media-player` that links against `libavcodec.so.60`, `libavformat.so.60`, and `libavutil.so.58` (all from `ffmpeg`):

```json
{
  "dependencies": [
    {
      "name": "ffmpeg",
      "dep_type": "Library (libavcodec.so.60) & Library (libavformat.so.60) & Library (libavutil.so.58)",
      "libraries": ["libavcodec.so.60", "libavformat.so.60", "libavutil.so.58"]
    }
  ]
}
```

Instead of three separate `ffmpeg` entries (one per library), a single consolidated entry is produced with the specific libraries enumerated.

</details>

<details><summary id="entry">Entry</summary>

The `main()` function in `src/main.rs` is the command-line interface. It uses `std::env::args()` to collect arguments and processes them with a `peekable` iterator. The function signature is:

```rust
fn main() -> Result<()>
```

The `anyhow::Result` return type allows the use of the `?` operator for error propagation, with any errors printed to stderr by Rust's default panic and error handling.

### Argument Parsing Logic

The parser uses a `while let Some(arg) = args.next()` loop with a `match` on each argument. It maintains two mutable strings: `manifest_path` and `output_dir`. The parsing follows these rules:

1. **Flags with values** (for example, `-m`, `-o`, `-j`, `-z`, `-p`, `-t`): The flag consumes the next argument as its value. For example, `-m manifest.json` sets `manifest_path` to `"manifest.json"`.

2. **Boolean flags** (for example, `-n`, `-f`, `-c`, `-s`, `-q`, `-d`, `-y`, `-k`, `-l`): These set environment variables using `unsafe { env::set_var(...) }`. The `unsafe` block is required because `set_var` is unsafe in the Rust standard library (it can cause data races in multi-threaded contexts, though `Outsider` is single-threaded).

3. **Positional arguments**: Any argument that does not start with `-` is treated as a positional argument. The first positional argument fills `manifest_path`, the second fills `output_dir`.

4. **`-h` / `--help`**: Prints the help message and exits with code 0.

5. **`-v` / `--version`**: Prints the version string `"Outsider 0.5.0"` and exits with code 0.

6. **`-a` / `--archive`**: Takes two positional arguments (staging directory and output package path) and runs `tar` directly to create an `.xcs` archive manually.

7. **`-x` / `--extract`**: Takes one or two positional arguments (package path or directory, and destination root). If the input is a directory, it scans for all `.xcs` files inside. Each package is extracted using `tar` first; if that fails, it falls back to `tar --zstd -xf` or `zstd -dc | tar -xf`. This provides compatibility with `tar.zstd` archives.

8. **`-w` / `--write`**: Takes two positional arguments (source directory and destination directory). Generates a `metadata.json` for the destination directory without archiving it. The package name is derived from the source directory filename.

9. **`-i` / `--inspect`**: Takes one positional argument (path to an `.xcs` package). Prints the file size, type (via the `file` command).

### Manifest Processing

After argument parsing, if both `manifest_path` and `output_dir` are non-empty, the program:

1. Reads the manifest file from disk using `fs::read_to_string`.
2. Deserializes it into a `Manifest` struct using `serde_json::from_str`.
3. Creates the output directory with `fs::create_dir_all`.
4. Iterates over each package in `manifest.packages` and calls `process(pkg, &output_dir)`.
5. If `OUS_QUIET` is not set, prints `"OK: <path>"` for each successfully built package.
6. If any package fails, the error is propagated with `?` and the program exits immediately (fail-fast behavior).

</details>

<details><summary id="core">Core</summary>

The library file contains all the data structures and functions that implement the build pipeline. Each function is designed to be independently callable, allowing for flexible composition and testing.

## Binary Reader

One of the most powerful features of `Outsider` is its **Binary Reader** — a zero-bloat dependency scanner that replaces the simplistic `ldd`-only approach with a sophisticated, two-phase analysis engine. Rather than including bulky disassembler libraries (like `libbfd`, `capstone`, or `llvm`) that would bloat the engine's binary, `Outsider` reads ELF binary files directly as raw byte streams and extracts meaningful dependency information from them.

The Binary Reader consists of four essential components, designed to work together like a mobile application's tightly integrated architecture:

### ELF Target Iterator

**Source function:** `elf(dir: &str) -> Result<Vec<PathBuf>>`

The ELF Target Iterator is the entry point for the Binary Reader. It performs a recursive directory traversal, typically scanning `system/bin` and `system/lib` directories inside the package staging area.

**Key design decisions:**

- **Magic byte validation**: Rather than relying on file extensions (which scripts and non-ELF files can fake), the iterator reads the first 4 bytes of every file it encounters. ELF binaries begin with the magic bytes `\x7f E L F` (`0x7f 0x45 0x4c 0x46`). Only files matching this signature are included in the results.
- **Script exclusion**: Shell scripts, Python scripts, and other text-based executables begin with `#!` (shebang) and are automatically filtered out by the ELF magic check. This prevents false positives from non-binary files.
- **Stack-based traversal**: The iterator uses an explicit `Vec<PathBuf>` as a stack for directory traversal rather than recursion, avoiding potential stack overflow on deeply nested directory trees.
- **Error tolerance**: Directories that cannot be read (permission denied, broken symlinks) are silently skipped. This ensures that a single inaccessible directory does not block the entire dependency scan.

**Algorithm:**

```text
1. If the root directory does not exist, return an empty vector
2. Push the root directory onto a stack
3. While the stack is not empty:
   a. Pop a directory from the stack
   b. Read all entries in the directory
   c. For each entry:
      - If it is a directory, push it onto the stack
      - If it is a file, open it and read the first 4 bytes
      - If the bytes match the ELF magic (0x7f, 0x45, 0x4c, 0x46), add the path to the result
4. Return the collected ELF binary paths
```

### Dynamic Symbol Parser

**Source function:** `strings(path: &Path) -> Result<Vec<String>>`

The Dynamic Symbol Parser is the core innovation of the Binary Reader. It reads an ELF binary as a raw byte stream and extracts printable ASCII strings of length >= 4 characters. This is the "zero-bloat" approach — no disassembler libraries, no heavy parsing infrastructure, just the binary's own byte content.

**What this catches:**

- **DT_NEEDED entries** from the `.dynstr` section of the ELF dynamic symbol table. These are the compile-time declared library dependencies that `ldd` also shows. The library names (e.g., `libfoo.so`, `libbar.so.1.0.0`) appear as printable strings in the `.dynstr` section.
- **`dlopen()` runtime calls**: When a binary calls `dlopen("/system/lib/libfoo.so", RTLD_NOW)` or `dlopen("libbar.so", RTLD_LAZY)`, the library path or name string is embedded in the `.rodata` section of the binary. `ldd` does NOT see these because they are not DT_NEEDED entries — they are runtime decisions. The Byte Stream Reader catches them because it scans all printable strings regardless of section.
- **`dlsym()` patterns**: Similar to `dlopen`, any library name or path string in `dlsym()` arguments is captured.
- **Embedded path strings**: Strings like `/system/lib/libquux.so` or `/usr/lib/libextra.so` that may be embedded in configuration data or string tables.

**Key design considerations:**

- **Minimum length filter**: Only strings of length >= 4 characters are captured. This filters out noise from single-byte characters and short garbage sequences that happen to align with printable ASCII.
- **Printable character set**: The parser collects bytes that are ASCII graphic (alphanumeric and punctuation) plus the characters `/`, `.`, `-`, `_`, and space. These are the characters commonly found in library paths and filenames.
- **Numeric noise filter**: Purely numeric strings (e.g., version numbers, addresses) are filtered out, as they cannot be library names.
- **Zero dependency footprint**: The entire string extraction is done with Rust's standard library file I/O plus basic byte iteration. No external parsing libraries are needed.
- **Streaming chunk-based reading**: To handle very large binaries (hundreds of megabytes or more) without exhausting memory on constrained devices, the function reads the file in 64KB chunks rather than loading the entire file at once. A carryover buffer ensures that strings split across chunk boundaries are correctly reassembled.

**Algorithm:**

```text
1. Open the binary file for reading
2. Initialize a carryover buffer (empty)
3. While not at EOF:
   a. Read 64KB chunk from file
   b. Combine carryover + current chunk
   c. Iterate over each byte in combined buffer:
      - If printable ASCII (graphic, /, ., -, _, or space):
        * Append to current string buffer
      - Otherwise (non-printable byte):
        * If current buffer length >= 4:
          - Convert to UTF-8 string
          - Skip if purely numeric
          - Add to results
        * Clear current buffer
   d. Save incomplete string (if any) to carryover for next iteration
4. Process any remaining carryover at EOF (same logic as above)
5. Return all extracted strings
```

### Dependency Matcher

**Source function:** `bds(dirs: &[[&str]]) -> Result<Vec<String>>`

The Zero-Bloat Dependency Matcher orchestrates Components 1 and 2 together. Given one or more directory paths, it:

1. Calls `elf()` on each directory to find all ELF binaries
2. For each binary found, calls `strings()` to extract printable strings
3. Passes the strings to `lib()` to extract library names
4. Aggregates all results into a single, sorted, deduplicated list

This function is designed to be called on multiple directories in sequence — first on `system/bin` (the package's own binaries), then on `system/lib` (any already-bundles libraries, for transitive dependency scanning).

**Core system library classification function:** `is_core_system_lib(lib_name: &str) -> bool`

Libraries that match known Cudane core prefixes are considered part of the base system and are NOT bundles into the package. This prevents unnecessary bloat while ensuring that third-party/supplemental libraries are still captured. The `CORE_SYSTEM_LIBS` constant contains an extensive list of known system libraries:

- **C standard library**: `libc.so`, `libm.so`, `libpthread.so`, `libdl.so`, `librt.so`, `libutil.so`
- **C++ runtime**: `libstdc++.so`, `libgcc_s.so`, `libatomic.so`, `libgomp.so`, `libquadmath.so`
- **Sanitizers**: `libasan.so`, `libubsan.so`, `liblsan.so`, `libtsan.so`
- **Compression**: `libz.so`, `libzstd.so`, `liblzma.so`, `libbz2.so`
- **Cryptography**: `libssl.so`, `libcrypto.so`
- **Regex**: `libpcre.so`, `libpcre2.so`
- **XML, Unicode**: `libexpat.so`, `libffi.so`, `libiconv.so`, `libintl.so`
- **Terminal**: `libncurses.so`, `libtinfo.so`, `libreadline.so`, `libhistory.so`
- **Dynamic linker**: `ld-linux`, `ld-musl`, `ld-musl-x86_64`
- **NSS**: `libnss_`, `libnss3.so`, `libnssutil3.so`
- **Security**: `libselinux.so`, `libsepol.so`, `libpam.so`, `libcap.so`
- **Filesystem**: `libacl.so`, `libattr.so`, `libmount.so`, `libblkid.so`, `libuuid.so`
- **JSON**: `libjson-c.so`
- **IPC**: `libdbus-1.so`
- **Graphics**: `libEGL.so`, `libGL.so`, `libdrm_*.so`, `libX11.so`, `libxcb.so`, `libwayland-*.so`
- **Audio**: `libpulse.so`, `libasound.so`, `libsndfile.so`
- **Fonts**: `libfreetype.so`, `libfontconfig.so`, `libharfbuzz.so`
- **Images**: `libpng`, `libjpeg`, `libwebp`, `libtiff`, `libgif`
- **Scripting**: `libpython`, `libperl.so`
- **Database**: `libsqlite3.so`
- **And many more...**

The matching uses both prefix checks and substring-in-name checks, so `libfoo.so.1.0.0` matches `libfoo.so` via the prefix test, and `libpthread-2.33.so` matches `libpthread.so` via a sliding window comparison.

### Metadata Synchronization & Dependency Signature

**Source function:** `compute(deps: &[[String]]) -> String`

This component is the final piece of the Binary Reader architecture. It generates a deterministic SHA-256 hash over the sorted external dependency list. This serves as a **cryptographic signature** that:

- Ensures the package's dependency metadata has not been tampered with
- Allows verification that all expected bundles libraries are present at install time
- Provides a unique fingerprint that can be compared against the manifest

**Algorithm:**

```shell
SHA-256( join("|", sorted_deps) )
```

Where `sorted_deps` is the alphabetically sorted list of all external library names discovered by the two-phase scan. The pipe character (`|`) is used as the delimiter because it is highly unlikely to appear in a library name.

The resulting 64-character hex string is stored in `PackageMetadata.depsig`.

This signature is computed during `process()` after the dependency resolution phase and before the metadata is written to disk. It becomes a permanent part of the package metadata, embedded inside the `.xcs` archive alongside `metadata.json`.

### Resolution by ldd

The true power of `Outsider`'s dependency analysis comes from its **two-phase approach**:

**Phase 1 (`ldd` — compile-time DT_NEEDED):**

The traditional `ldd` command is still used as the first phase. It captures all **compile-time declared shared library dependencies** — i.e., the `DT_NEEDED` entries in the ELF dynamic section. These are the libraries listed in the binary's `.dynamic` section.

`ldd` works by:

1. Reading the ELF binary's `.dynamic` section to find `DT_NEEDED` entries
2. Resolving each entry name (e.g., `libfoo.so.6`) to an actual file path using the system's library search paths (`/etc/ld.so.conf`, `LD_LIBRARY_PATH`, default paths like `/usr/lib`, `/lib`)
3. Recursively repeating the process for each resolved library

The output is a line-by-line listing showing each library name, its resolved path, and its load address. For example:

```shell
        linux-vdso.so.1 (0x00007ffd5a3e0000)
        libfoo.so.6 => /usr/lib/x86_64-linux-gnu/libfoo.so.6 (0x00007f8a12340000)
        libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f8a12000000)
        /lib64/ld-linux-x86-64.so.2 (0x00007f8a12400000)
```

**Phase 2 (Binary Reader — runtime dlopen/dlsym):**

The Binary Reader opens each ELF binary (both executables and shared libraries) and reads it as a raw byte stream, extracting **all** printable strings of length >= 4 characters. This catches:

- **Runtime dynamic loading**: Libraries loaded via `dlopen()`, `dlsym()`, or similar mechanisms. These are NOT visible to `ldd` because they are not declared in `DT_NEEDED`.
- **Plugin architectures**: Programs that dynamically discover and load plugins (e.g., libpurple protocol plugins, Apache modules, GStreamer plugins) often embed plugin library names in string tables that are readable via byte scanning.
- **Configuration-embedded paths**: Path strings embedded in `.rodata` or configuration data that reference shared libraries.
- **Transitive dependencies**: When scanning already-bundles libraries in `system/lib`, the Binary Reader can discover dependencies of those libraries, which `ldd` may not resolve if the libraries are not on the system's library path.

**Why two phases are necessary:**

| Aspect | `ldd` Only | Binary Reader Only | Combined |
| -------- | ----------- | ------------------- | ---------- |
| Compile-time DT_NEEDED | ✅ | ✅ (via .dynstr) | ✅ |
| Runtime dlopen/dlsym | ❌ | ✅ | ✅ |
| Recursive transitive deps | ✅ | ✅ | ✅ |
| Path resolution to real files | ✅ | ❌ (names only) | ✅ |
| Cross-platform compatibility | ✅ | ✅ | ✅ |
| Works with statically-linked bins | ❌ | ✅ (finds strings) | ✅ |

</details>

<details><summary id="starting">Starting</summary>

Build the engine (GNU-based distributions):

```shell
cargo build --release
```

If you are on Cudane:

```shell
# 1. Install nightly:
rustup toolchain install nightly

# 2. Add the nightly toolchain library for rust-src:
rustup component add rust-src --toolchain nightly

# 3. Build:
rustup run nightly cargo -Zjson-target-spec -Zbuild-std build --release --target x86_64-pc-linux-musl.json
```

Run with a manifest and an output directory:

```shell
./target/release/ous manifest.json output
```

Optional flag:

```shell
./target/release/ous manifest.json output --no-auto
```

> [!TIP]
> When `--no-auto` is passed, the program sets the `OUS_NO_AUTO` environment variable to disable the auto behaviors for empty `build_cmd` and `install_cmd` fields.

## Building Variables

### Building only / Direct destination

in `install_cmd`, type:

```shell
DESTDIR=/your/dest/path
```

### Building a package (Building + Archiving)

in `install_cmd`, type:

```shell
DESTDIR=$CUDANE_DEST
```

> [!TIP]
> **Why `$CUDANE_DEST`**?
> Outsider uses this variable to configure the package setup by your `build_cmd` + `install_cmd` commands, and passing the output to the `archive` function.

> [!WARNING]
> Ignoring or not setting a destination variable causes the engine to install the package inside the temporary working folder as the final system destination, so better for you choose one of these two options.

</details>

<details><summary id="guide">Guide</summary>

For `Outsider` packages to be truly 100% independent and benefit fully from the Binary Reader's dependency freezing, the staging directory (`pkg/`) must follow a strict root directory tree structure. This structure mirrors the Cudane Linux filesystem layout.

## Package Root Layout

```shell
pkg/                                    # Package staging root
├── metadata.json                       # Generated automatically (populated by Outsider)
├── system/
│   ├── bin/                            # Executable binaries
│   │   ├── my-app                      # Main executable
│   │   ├── helper-tool                 # Associated utility
│   │   └── ...                         # Any other binaries
│   ├── lib/                            # Shared libraries (frozen here by Binary Reader)
│   │   ├── libfoo.so.1.0.0             # Discovered + bundles automatically
│   │   ├── libbar.so.2                 # Discovered + bundles automatically
│   │   └── ...                         # Any other supplemental libs
│   ├── dinit.d/                        # Dinit service files (optional)
│   │   └── my-app.service              # Service definition
│   ├── share/                          # Read-only architecture-independent data
│   │   ├── doc/                        # Documentation
│   │   │   └── my-app/
│   │   ├── man/                        # Manual pages
│   │   │   └── man1/
│   │   └── licenses/                   # License files
│   │       └── MIT
│   ├── etc/                            # Configuration files (defaults)
│   │   └── my-app.conf
│   └── include/                        # Header files (for -dev packages)
│       └── my-app/
│           └── my-app.h
├── config/                             # Package-specific mutable config (optional)
│   └── my-app/
└── data/                               # Package-specific mutable data (optional)
    └── my-app/
```

## Rules for Organizing Packages

1. **Everything under `system/`**: All system-level content belongs inside `system/`. This mirrors the Cudane root filesystem where `/system` is the base prefix.

2. **Binaries go in `system/bin/`**: All executables must reside in `system/bin/`. The Binary Reader scans this directory for ELF binaries to analyze their dependencies.

3. **Libraries go in `system/lib/`**: All shared libraries must be placed in `system/lib/`. The Binary Reader:
   - Copies discovered supplemental libraries into this directory.
   - Also scans this directory for transitive dependency analysis (already-bundles libraries may themselves depend on other libraries).

4. **`.xcs` sub-packages go anywhere**: Embedded `.xcs` packages are discovered recursively by the `components()` function.

5. **Service files go in `system/dinit.d/`**: Dinit service files placed here are automatically detected and listed in the package metadata.

6. **Symlinks follow the same structure**: The `links` field in the manifest should reference paths relative to the given structure (e.g., `"system/lib/libexample.so.1": "system/lib/libexample.so"`).

## Automating Organization with Install Commands

The simplest way to ensure correct organization is to use install commands that target the `CUDANE_DEST` environment variable. For example:

```json
{
  "name": "my-app",
  "version": "1.0.0",
  "build_type": "make",
  "build_cmd": "make -j$(nproc)",
  "install_cmd": "make DESTDIR=$CUDANE_DEST install prefix=/system",
  "links": {
    "system/bin/my-app": "system/bin/my-app-v1"
  }
}
```

The `$CUDANE_DEST` environment variable is automatically set by `Outsider` to the absolute path of the package staging directory (`pkg/`). By installing with `DESTDIR=$CUDANE_DEST prefix=/system`, the build system places files under `$CUDANE_DEST/system/`, which becomes `pkg/system/` — exactly matching the expected layout.

## Extraction Fallback Compatibility

`Outsider` supports a fallback extraction chain for packages that may have been created with alternative compression methods:

```shell

# Primary (tar + zstd):
tar --zstd -xf <package.xcs> -C <target>

# Fallback (zstd pipe + tar):
zstd -dc <package.xcs> | tar -xf - -C <target>
```

## Compression Algorithm Comparison

| Algorithm | Outsider Default? | Speed | Ratio | Best For |
| ----------- | ---------------- | ------- | ------- | ---------- |
| **zstd** | ✅ | Fast (native) | Good | General purpose; best overall trade-off |
| **gzip** | ❌ | Medium | Medium | Maximum compatibility (older systems) |
| **lzo** | ❌ | Very fast | Low | Fast boot times (live systems) |
| **lz4** | ❌ | Fastest | Lowest | Very fast random access |
| **xz** | ❌ | Slow | Best | Maximum compression for distribution |

The default `zstd` at level 3 provides roughly 2-3× compression on typical mixed data with near-native decompression speed, making it ideal for package distribution.

---

## fetch - Source Acquisition

```rust
pub fn fetch(src: &str, dir: &str) -> Result<()>
```

This function is responsible for getting source code into the workspace. It takes a source string and a destination directory path.

**Logic:**

1. **`file://` prefix stripping**: If the source starts with `file://`, the prefix is stripped to get the actual filesystem path. This allows users to write `"source": "file:///home/user/project"` and have it treated as a local path.

2. **Local path check**: The function checks if the (possibly stripped) path exists on the local filesystem using `Path::new(src_path).exists()`. If it does, the source is handled locally:
    - **If it is a directory**: A recursive copy is performed using the inner `copy_dir_recursive()` closure. This closure creates the destination directory, iterates over all entries in the source, and recursively copies directories or directly copies files. The closure is defined inline within `fetch()` to keep the recursive logic scoped.
    - **If it is a file**: The destination directory is created, and the single file is copied into it, preserving the original filename.

3. **Git clone**: If the source ends with `.git`, a shallow clone is performed:

```shell
git clone --depth 1 <src> <dir>
```

The `--depth 1` flag limits the clone to only the most recent commit, minimizing bandwidth and disk usage. If the clone fails, an error is returned.

1. **Remote archive download**: For all other sources (assumed to be URLs pointing to compressed archives), the function:
    - Determines the archive name based on the file extension: `.xz` becomes `temp.tar.xz`, `.bz2` becomes `temp.tar.bz2`, everything else becomes `temp.tar.gz`.
    - Downloads the file using `curl -fSL -o <archive_path> <src>`. The flags mean: `-f` (fail silently on HTTP errors), `-S` (show errors), `-L` (follow redirects).
    - Extracts the archive using `tar` with the appropriate decompression flag: `-xJf` for `.xz`, `-xjf` for `.bz2`, `-xzf` for `.gz`. The `--strip-components=1` flag removes the top-level directory from the archive, so the contents are placed directly in the destination directory.
    - Deletes the downloaded archive file after extraction.
    - Returns an error if either `curl` or `tar` fails.

## build - Compilation Execution

```rust
pub fn build(pkg: &Package, dir: &str) -> Result<String>
```

This function executes the build command for a package and returns the captured build log as a `String`. The return value is used by the dependency scanner (though the current codebase does not perform deep dependency scanning from logs — the log is captured for potential future use or external tooling).

**Decision tree:**

1. **Skip keywords**: If `build_cmd` (trimmed) equals `"none"`, `"skip"`, or `"nothing"` (case-insensitive comparison via `eq_ignore_ascii_case`), the function returns an empty string immediately. No build occurs.

2. **Empty command with `OUS_NO_AUTO`**: If `build_cmd` is empty and the `OUS_NO_AUTO` environment variable is set, the function returns an empty string. This gives users explicit control to disable automatic behaviors.

3. **Empty command with `build_type == "rust"`**: The automatic Rust build is triggered:
    - The `RUSTFLAGS` environment variable is set to Cudane-specific values: linker is `clang`, target is `x86_64-pc-linux-musl`, sysroot is `/system`, and static CRT is enabled.
    - `cargo build --target x86_64-unknown-linux-musl --release` is executed in the source directory.
    - Both stdout and stderr are captured into a single `log_content` string.
    - The log is written to `capture.log` in the source directory.
    - If the build succeeds, the log content is returned. If it fails, the error includes the full log output for debugging.

4. **Empty command with other build types**: Returns an empty string (no-op).

5. **Non-empty command**: The command is executed via:

```shell
sh -c "(<command>) 2>&1 | tee capture.log"
```

The command is wrapped in parentheses to capture all output, `2>&1` redirects stderr to stdout, and `tee` writes the output to `capture.log` while also displaying it (though in a non-interactive context, the display effect is minimal). After execution, the log file is read into memory and deleted. If the command succeeds, the log content is returned; otherwise, an error is returned.

## install - Staging Installation

```rust
pub fn install(pkg: &Package, src: &str, dest: &str) -> Result<()>
```

This function installs built artifacts from the source directory into the package staging directory (`dest`). The `dest` path is the `pkg` subdirectory under the workspace.

**Decision tree:**

1. **Skip keywords**: If `install_cmd` (trimmed) equals `"none"`, `"skip"`, or `"nothing"` (case-insensitive), the install command is skipped. However, any symlinks declared in `pkg.links` are still created. This allows packages to declare symlinks without running any install command.

2. **Empty command with `OUS_NO_AUTO`**: Same as above — install is skipped, symlinks are still processed.

3. **Empty command with `build_type == "rust"`**: The automatic Rust install is triggered:
    - The function looks for `target/release/` inside the source directory.
    - If the directory exists, it iterates over all entries.
    - For each file (not directory) in `target/release/`, it copies the file to the package staging directory, overwriting any existing file with the same name.
    - After copying, any symlinks in `pkg.links` are created.

4. **Non-empty command**: The command is executed via:

```shell
sh -c "<install_cmd>"
```

The `CUDANE_DEST` environment variable is set to the package staging directory path, and `CUDANE_PREFIX` is set to the package's prefix (e.g., `"system"`, `"usr"`). The command runs with the source directory as its working directory (`current_dir(src)`). This allows install commands to reference `$CUDANE_DEST` as the target root and `$CUDANE_PREFIX` to target the correct installation prefix. After the command completes, any symlinks in `pkg.links` are created.

1. **Empty command with other build types**: Falls through to execute the empty string as a command (which effectively does nothing in a shell), then processes symlinks.

## symlink - Symbolic Link Management

```rust
pub fn symlink(target: &str, link_path: &str, root_dir: &str) -> Result<()>
```

This function creates a symbolic link inside the package staging directory.

**Logic:**

1. **Path sanitization**: The `link_path` is stripped of any leading `/` using `trim_start_matches('/')`. This ensures the link path is relative to the staging root, preventing accidental absolute paths that could escape the staging directory.

2. **Full path construction**: The full link path is constructed as `{root_dir}/{safe_link_path}`.

3. **Parent directory creation**: The parent directory of the link path is created using `fs::create_dir_all`. This ensures that nested link paths (for example, `system/lib/libexample.so`) work even if the intermediate directories do not exist yet.

4. **Existing file removal**: Any existing file at the link path is removed with `fs::remove_file`. This prevents "File exists" errors when updating symlinks.

5. **Symlink creation**: The symlink is created using `std::osunix::fs::symlink(target, &full_link_path)`. This is a Unix-specific system call that creates a symbolic link. The `target` is stored as-is — it can be relative or absolute, depending on what the manifest specifies.

## hash - Cryptographic Directory Checksum

```rust
pub fn hash(dir: &str) -> Result<String>
```

This function computes a SHA-256 hash of the entire contents of a directory. The hash is deterministic — the same directory contents always produce the same hash.

**Implementation:**

1. The function runs `tar -cf - -C <dir> .` which creates a tar archive of the directory contents on stdout. The `-C <dir>` flag changes to the target directory before archiving, and `.` includes everything in that directory.

2. The raw byte stream from `tar` is piped through a SHA-256 hasher (`sha2::Sha256::digest()`).

3. The resulting 32-byte hash is converted to a 64-character hexadecimal string using `format!("{:02x}", b)` for each byte.

The use of `tar` as an intermediate format ensures that the hash covers the complete directory structure, including file contents, permissions, and metadata that `tar` preserves. This provides a reliable fingerprint for verifying package integrity.

## license -  License Scanner

```rust
fn license(src_dir: &str) -> String
```

This function attempts to automatically determine the software license of a package by scanning its source directory. It is a private function (no `pub` visibility) called internally by `process()`.

**Logic:**

1. **File name matching**: The function checks for files with specific names (case-insensitive after uppercasing): `LICENSE`, `COPYING`, `LICENSE.MD`, `COPYING.MD`, `MIT-LICENSE`, `UNLICENSE`. These are the most common license file names used in open-source projects.

2. **Regex pattern matching**: For each matching file, the content is read and scanned against a comprehensive regex pattern:

```shell
(?i)(gnu\s+general\s+public\s+license|gpl|lgpl|agpl|apache|mit|bsd|mpl|\
mozilla\s+public\s+license|unlicense|isc)\s*(v(?:ersion)?\s*\d+(?:\.\d+)?|\
\d+[[---]]clause|\d+(?:\.\d+)?\b)?
```

This regex captures:
    - The license name (group 1): Supports GPL, LGPL, AGPL, Apache, MIT, BSD, MPL, Mozilla Public License, Unlicense, ISC, and their variations.
    - The version or clause (group 2, optional): Captures version numbers (for example, `v2`, `version 3`, `2.0`), clause specifications (for example, `2-clause`, `3-clause`), or bare version numbers.

1. **Name formatting**: The captured license name is formatted:
    - If the name is 4 characters or fewer (for example, `MIT`, `GPL`, `BSD`), it is uppercased.
    - Otherwise, it is converted to Title Case (each word capitalized).
    - If a version is captured, it is appended: versions starting with `v` are formatted as `v<number>`, others are appended as-is.

2. **Fallback to first line**: If no regex match is found, the function reads the first non-empty line of the license file, strips common comment characters (`*`, `#`, `/`), and if the result is non-empty and under 60 characters, returns it as the license string.

3. **Default**: If no license files are found or none of them yield a recognizable license string, the function returns `"Unknown"`.

## meta - Metadata  Writer

```rust
pub fn meta(meta: &PackageMetadata, dest: &str) -> Result<()>
```

This function writes a `PackageMetadata` struct to disk as a JSON file named `metadata.json` inside the specified destination directory.

**Implementation:**

1. The `PackageMetadata` struct is serialized to a pretty-printed JSON string using `serde_json::to_string_pretty(meta)`.

2. The JSON string is written to `{dest}/metadata.json` using `fs::write`.

The `metadata.json` file becomes part of the package archive and can be read by downstream tools (like the MCX Package Manager) to understand the package provenance, dependencies, and integrity.

## index - Repository Index Aggregator

```rust
pub fn index(index_root: &str, meta: &PackageMetadata) -> Result<()>
```

This function maintains a repository-wide index of all built packages. The index is stored as `index.json` in the output directory.

**Logic:**

1. **Index file location**: The index is stored at `{index_root}/index.json`. The directory is created if it does not exist.

2. **Existing index loading**: If the index file already exists, it is read and deserialized into a `Vec<PackageMetadata>`. If deserialization fails (for example, corrupted file), an empty vector is used as a fallback.

3. **Entry matching**: The function searches for an existing entry with the same `pkg_name` and `version` as the new metadata. If found:
    - If the existing entry is identical to the new one (using `PartialEq` comparison), the function returns early — no update needed.
    - If different, the existing entry is replaced with the new metadata.

4. **New entry**: If no matching entry exists, the new metadata is appended to the vector.

5. **Sorting**: The entries are sorted by `(pkg_name, version)` using `.sort_by()` with a tuple comparison. This ensures deterministic ordering in the index file.

6. **Writing**: The sorted vector is serialized to pretty-printed JSON and written to `index.json`.

This mechanism allows repository management tools to quickly discover all available packages and their versions without scanning individual `.xcs` files.

## archive - Zstd Compression

```rust
pub fn archive(dest: &str, out: &str) -> Result<()>
```

This function compresses a package staging directory into an `.xcs` file using `tar -c -C <dest> . | zstd -3 <out>` with Zstandard compression.

**Implementation:**

1. Any existing file at the output path is removed (to prevent `tar` from complaining about overwriting).

2. The `tar` command is invoked with:

```shell
tar -c -C <dest> . | zstd -3 <out>
```

- `<dest>`: The package staging directory to compress.
- `<out>`: The output `.xcs` file path.

1. If `tar` succeeds, the function returns `Ok(())`. Otherwise, it returns an error.

The `.xcs` extension is a convention used by Cudane for `.xcs` Packages compressed with Zstandard. The `-noappend` flag ensures each build produces a clean, independent archive.

## Build / Resume

`Outsider` features a **resumable build pipeline** that allows it to continue from the exact byte it stopped on, provided the workspace directory (`.ous/`) has not been deleted.

### State File

Each package's workspace contains a state file at `.ous/{package_name}/.state.json`:

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
struct BuildProgress {
    completed_steps: HashSet<String>,
}
```

The following step identifiers are tracked:

| Step | Description |
| --- | --- |
| `fetch` | Source code successfully fetched into `src/` |
| `build` | Build command executed successfully |
| `install` | Artifacts installed into `pkg/` |
| `hash` | Checksums computed for `pkg/` |
| `metadata` | `metadata.json` written and index updated |
| `archive` | Final `.xcs` package compressed |

### Resume Flow

1. When `process()` starts, it checks for `.state.json` in the package workspace.
2. If the file exists and `OUS_CLEAN` is **not** set, the state is loaded and only steps **not** in `completed_steps` are executed.
3. After each successful step, the state file is updated atomically (written via `serde_json` + `fs::write`).
4. Intermediate outputs are persisted:
   - Build log → `build_log.txt`
   - Checksums → `checksums.json`

### Overwrite / Force

- **`-f` / `--force`** (sets `OUS_FORCE=1`): Ignores the existing `.xcs` output file and rebuilds from scratch (or resumes from workspace state).
- **`-c` / `--clean`** (sets `OUS_CLEAN=1`): Deletes the entire workspace directory before starting, ensuring a completely fresh build.

### Use Case

If a build is interrupted (power failure, network timeout, crash) after the fetch and build steps but before archiving, running the same command again will:

1. Detect the existing state file
2. Skip fetch (already complete)
3. Skip build (already complete)
4. Re-run install → hash → metadata → archive

This avoids re-downloading source and recompiling, saving significant time on large packages.

## process - The Engine Processor

```rust
pub fn process(pkg: &Package, out_dir: &str) -> Result<String>
```

This is the main orchestrator function that ties together the entire build pipeline for a single package. It is called by `main.rs` for each package in the manifest. It features a **resume/overwrite mechanism** that allows builds to continue from where they left off after interruption.

**Walkthrough:**

1. **Output directory resolution**: The function gets the current working directory and joins it with the provided `out_dir` to create an absolute output directory path. This ensures that all paths are absolute and unambiguous.

2. **Short-circuit check**: The function constructs the expected output path: `{absolute_out_dir}/{name}-{version}.xcs`. If this file already exists and the `OUS_FORCE` environment variable is **not** set, the function returns immediately with the existing path. This prevents unnecessary rebuilds of already-built packages. To force a rebuild, users pass `--force` (which sets `OUS_FORCE=1`).

3. **Workspace directory setup**: The workspace is at `.ous/{package_name}`. Inside this:
    - `src/` — Where source code is fetched and built.
    - `pkg/` — Where installed artifacts are staged before archiving.
    - `.state.json` — State file tracking completed build steps (see [Build / Resume]).
    - `build_log.txt` — Persisted build log for dependency re-scanning on resume.
    - `checksums.json` — Persisted checksum results for resume.

4. **State loading / workspace initialization**:
    - If `OUS_CLEAN` is set **or** no `.state.json` exists: the workspace is deleted and created fresh.
    - Otherwise: the existing state file is loaded, and only incomplete steps are re-executed.

5. **Source fetch**: `fetch(&pkg.source, src_str)` is called to populate the `src/` directory if the `fetch` step is not marked complete. If the step was already completed (from a previous partial run), it is skipped.

6. **Build execution**: `build(pkg, src_str)` is called to compile the source if the `build` step is not marked complete. The build log is saved to `build_log.txt` for potential resume. On resume, if the build is complete, the log is read from disk.

7. **Installation**: `install(pkg, src_str, root_str)` is called to copy built artifacts into the `pkg/` staging directory if the `install` step is not marked complete.

8. **Directory hashing**: `hash(root_str)` computes checksums (SHA-256, SHA-1, MD5) of the entire staging directory if the `hash` step is not marked complete. Results are persisted to `checksums.json`.

9. **Metadata construction**: `mtd()` generates the `PackageMetadata` struct — which internally calls:
    - `scan()` for dependency resolution (including the two-phase Binary Reader scan and the **Consolidation** feature)
    - `files()` for file listing
    - `provides()` for library discovery
    - `license()` for license detection

10. **Metadata writing**: `write(&metadata, root_str)` writes `metadata.json` into the staging directory.

11. **Index update**: `index()` appends or updates the entry in `index.json` in the output root.

12. **Archiving**: `archive(root_str, &final_path)` compresses the staging directory into the final `.xcs` file.

13. **State finalization**: After archiving, all steps are marked complete in `.state.json`. The next run will find the `.xcs` file and short-circuit.

</details>

<details><summary id="cli">CLI</summary>

The CLI supports the following flags, each of which sets a corresponding environment variable or triggers a specific action:

| Flag | Long Flag | Environment Variable | Action |
| ------ | ----------- | --------------------- | -------- |
| `-h` | `--help` | --- | Print help message and exit |
| `-v` | `--version` | --- | Print version and exit |
| `-a` | `--archive` | --- | Manual archive mode (takes 2 args: staging dir, output path) |
| `-x` | `--extract` | --- | Extract mode (takes 1-2 args: package or dir, destination) |
| `-w` | `--write` | --- | Write metadata mode (takes 2 args: src dir, dest dir) |
| `-g` | `--hash-type` | `OUS_HASH_TYPE` | Set the checksum algorithm for package metadata (default: sha256)" |
| `-i` | `--inspect` | --- | Inspect Engine (takes 1 arg: package path) |
| `-n` | `--no-auto` | `OUS_NO_AUTO=1` | Disable automatic build and install behaviors |
| `-f` | `--force` | `OUS_FORCE=1` | Overwrite existing `.xcs` packages |
| `-c` | `--clean` | `OUS_CLEAN=1` | Clean workspace before building |
| `-s` | `--strict` | `OUS_STRICT=1` | Fail immediately on dependency mapping errors |
| `-q` | `--quiet` | `OUS_QUIET=1` | Suppress standard output messages |
| `-d` | `--debug` | `OUS_DEBUG=1` | Enable verbose debug logging |
| `-y` | `--yes` | `OUS_ASSUME_YES=1` | Assume yes to all prompts |
| `-k` | `--keep-src` | `OUS_KEEP_SRC=1` | Do not delete source directory after build |
| `-l` | `--parallel` | `OUS_PARALLEL=1` | Enable parallel package processing |
| `-j` | `--jobs <NUM>` | `OUS_JOBS=<NUM>` | Set number of parallel make jobs |
| `-z` | `--zstd-level <NUM>` | `OUS_ZSTD_LEVEL=<NUM>` | Set zstd compression level for tar |
| `-p` | `--project <DIR>` | `OUS_PROJECT_WORKSPACE=<DIR>` | Define custom project or workspace directory |
| `-t` | `--target <ARCH>` | `OUS_TARGET=<ARCH>` | Define target architecture |
| `-m` | `--manifest <FILE>` | --- | Path to manifest.json |
| `-o` | `--output <DIR>` | --- | Path to output directory |

The environment variables are set using `unsafe { env::set_var(...) }` because the Rust standard library marks `set_var` as unsafe (it can cause data races in multi-threaded contexts). Since `Outsider` processes packages sequentially in a single thread, this is safe in practice.

</details>

<details><summary id="format">Format</summary>

## What is XCS?

The `.xcs` Package is a **`zstd` Archive** compressed with `tar` at compression level 3. Zstandard is a highly compressed archives for Linux that provides:

- **Lowest Compression Sizes**: With level `3`.
- **Metadata Preservation**: File permissions, ownership, and timestamps are preserved.
- **Modern Storage**: Zstandard is designed for modern-enough systems.

Inside every `.xcs` archive, there is a `metadata.json` file at the root that contains the `PackageMetadata` structure. This allows downstream tools to inspect package properties without extracting the entire archive.

</details>

<details><summary id="licenses">Licenses</summary>

The `license()` function implements a heuristic scanner that works as follows:

1. **File enumeration**: The function reads all entries in the source directory and checks their names (uppercased) against a list of known license file names: `LICENSE`, `COPYING`, `LICENSE.MD`, `COPYING.MD`, `MIT-LICENSE`, `UNLICENSE`.

2. **Content scanning**: For each matching file, the content is read and scanned with a regex pattern designed to match common open-source license declarations. The regex is case-insensitive and captures:
    - License names: GPL, LGPL, AGPL, Apache, MIT, BSD, MPL, Mozilla Public License, Unlicense, ISC.
    - Version and clause information: Version numbers (for example, `v2`, `version 3`), clause specifications (for example, `2-clause`, `3-clause`), or bare numbers.

3. **Name normalization**: The captured license name is normalized:
    - Short names (4 characters or fewer) are uppercased: `mit` becomes `MIT`, `gpl` becomes `GPL`.
    - Longer names are Title Cased: `gnu general public license` becomes `GNU General Public License`.
    - Version information is appended: `GPL v3`, `MIT`, `BSD 2-Clause`.

4. **Fallback**: If no regex match is found, the function reads the first non-empty line of the license file, strips comment characters (`*`, `#`, `/`), and if the result is under 60 characters, returns it as-is.

5. **Default**: If no license files are found or none yield a recognizable string, `"Unknown"` is returned.

</details>

<details><summary id="indexing">Indexing</summary>

The `index()` function maintains a cumulative index of all packages built into a given output directory. The index is stored as `index.json` and follows this logic:

1. **Load existing index**: The function reads `{index_root}/index.json` if it exists. If the file is missing or corrupted, an empty list is used.

2. **Match by identity**: The function searches for an existing entry with the same `pkg_name` and `version`. This is a compound key — both fields must match for an entry to be considered the same package.

3. **Update or append**:
    - If a matching entry is found and it is identical to the new metadata (using `PartialEq`), no changes are made.
    - If a matching entry is found but different, it is replaced with the new metadata.
    - If no matching entry is found, the new metadata is appended.

4. **Sort**: The entries are sorted by `(pkg_name, version)` for deterministic ordering.

5. **Write**: The sorted list is serialized to pretty-printed JSON and written to `index.json`.

This design allows the index to be incrementally updated as new packages are built, without requiring a full rebuild of the index each time.

</details>

<details><summary id="multiarch">Multi-Architecture Pipeline</summary>

`Outsider` supports building packages for multiple target architectures from a single manifest. The pipeline is driven by the `CUDANE_TARGETS` environment variable and the included `pipeline.sh` script.

### Usage

```shell
export CUDANE_TARGETS="x86_64-pc-linux-musl,aarch64-unknown-linux-musl"
./pipeline.sh manifest.json
```

For each target, the pipeline:

1. Sets `OUS_TARGET` to the target triple, which is read by the engine's auto-build path to pass `--target <triple>` to `cargo`.
2. Creates `output/<arch>/` for built `.xcs` packages.
3. Writes `index.<arch>.json` with arch-specific package metadata.
4. Moves packages into `pool/<arch>/<name>/` for organized storage.

### Target Spec Files

Rust `.json` target specs define the LLVM target, data layout, and linker for each architecture:

| File | Architecture |
| --- | --- |
| `x86_64-pc-linux-musl.json` | amd64, x86-64-v3, musl |
| `aarch64-unknown-linux-musl.json` | arm64, armv8-a, musl |

To add a new architecture, create the target spec `.json` file and add its triple to `CUDANE_TARGETS`.

### Cargo Configuration

Each target has a corresponding section in `cargo/config.toml` with target-specific `rustflags`:

```toml
[target.x86_64-pc-linux-musl]
linker = "clang"
rustflags = ["-C", "target-cpu=x86-64-v3", ...]

[target.aarch64-unknown-linux-musl]
linker = "clang"
rustflags = ["-C", "target-cpu=armv8-a", ...]
```

### Engine Integration

- **`Package.arch`** — Set per-package in the manifest (defaults to `"native"` for backward compatibility).
- **`OUS_TARGET`** — Environment variable read by the auto Rust build to determine the `--target` triple.
- **Arch-aware workspace** — Each arch gets its own workspace at `.os/<pkg>/<arch>/` to avoid rebuild conflicts when building the same package for multiple architectures.
- **Arch-specific index** — `index()` writes `index.<arch>.json` when the architecture is set, keeping per-arch metadata separate.

</details>

<details><summary id="requirements">Requirements</summary>

Because `Outsider` delegates specialized operations to highly optimized system-level utilities, the build host must have the following tools installed and accessible in `$PATH`:

| Dependency | Purpose within Outsider |
| --- | --- |
| **Rust Toolchain** | Required to build the core `Outsider` compiler itself, alongside fallback execution of `cargo` for `rust` build-types. |
| **`sh` (POSIX Shell)** | Executing custom user-defined build and install command hooks dynamically. |
| **`git`** | Managing external source code repositories via rapid depth-restricted clones. |
| **`curl`** | Handling remote archive downloads with robust error and status tracking. |
| **`tar`** | Extracting source assets, staging layout tracking, and processing intermediate streams. |
| **`zstd`** | Compression backend used by `tar` |
| **`ldd`** | Phase 1 of the two-phase dependency scan: captures compile-time DT_NEEDED entries. |
| **`file`** | Used by the `-i`/`--inspect` flag to determine the file type of a package. |

</details>

<details><summary id="cudane">Cudane</summary>

When building packages to run natively inside the **Cudane** landscape, compilation routines must adhere strictly to the target distribution standardized structural layouts and compiler configurations.

### 1. Mandatory Compilation Flags

To protect system integrity and match Cudane custom root-directory paradigm, software components should not target conventional paths like `/usr` or `/usr/local`. Instead, specify the official base prefix explicitly:

```shell
--prefix=/system
```

### 2. Clang Toolchain Enforcement

Cudane eliminates standard GCC assumptions in favor of a modern, strict LLVM and Clang foundation backed by the lightweight `musl` C library. Every source compilation command block initialized via a manifest must declare the tracking environment variables and direct cross-compilation target parameters:

```shell
CC=\"clang --target x86_64-pc-linux-musl -march=x86-64-v3 -O3 -flto=full -static\" --sysroot=$DESTDIR/system
```

* `CC="clang"`/`CXX="clang"`: Defines the primary compiler engine embedded with aggressive target constraints passed as an indivisible string:
* `--target=x86_64-pc-linux-musl`: Mandates code generation tailored strictly to the highly performant `musl` C library runtime engine on standard PC architecture instead of standard `glibc`, passed as a single token with an equals sign (`=`) to prevent host flag reordering.
* `-march=x86-64-v3`: Forces the compiler to exploit advanced microarchitecture extensions (including AVX, AVX2, BMI2, and SSE4.2), doubling general-purpose registers to eliminate memory spilling and maximize raw execution speed.
* `-O3`: Activates aggressive compiler optimization loops, vectorizing mathematical operations and restructuring the binary layout for extreme runtime performance.
* `-flto=full`: Enables full Link-Time Optimization across both compile and link phases, allowing LLVM to analyze the entire codebase as a single unit to eliminate dead code and aggressively debloat dependencies.
* `-static`: Guarantees absolute static linking with `musl-libc`, outputting a sovereign, self-contained binary entirely free of runtime dynamic dependencies.
* `--sysroot=$DESTDIR/system`: Injected directly inside the compiler definition string to redirect the Clang link editor and preprocessor header mechanics to the Systemfs filesystem tree (`/system` - alternative to `/usr`).

Make sure that you have been set up the `$DESTDIR` variable befor start building, see [[`Starting`](#starting)] section for more informations about setting the building variable.

### 3. Rust Architecture

Cudane's architecture is not shipped with Rust's built-in target definitions, so you must use the integrated target spec files. These JSON files define the LLVM target, data layout, and linker for each architecture. They are required when building Rust packages for Cudane.

#### x86_64 (amd64)

File: `x86_64-pc-linux-musl.json`
```json
{
  "arch": "x86_64",
  "cpu": "x86-64-v3",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
  "env": "musl",
  "executables": true,
  "linker": "clang",
  "linker-flavor": "gnu-cc",
  "llvm-target": "x86_64-pc-linux-musl",
  "max-atomic-width": 64,
  "os": "linux",
  "position-independent-executables": true,
  "crt-static-default": true,
  "crt-static-respected": true,
  "target-pointer-width": "64",
  "vendor": "pc"
}
```

#### aarch64 (arm64)

File: `aarch64-unknown-linux-musl.json`
```json
{
  "arch": "aarch64",
  "cpu": "armv8-a",
  "data-layout": "e-m:e-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128",
  "env": "musl",
  "executables": true,
  "linker": "clang",
  "linker-flavor": "gnu-cc",
  "llvm-target": "aarch64-unknown-linux-musl",
  "max-atomic-width": 128,
  "os": "linux",
  "position-independent-executables": true,
  "crt-static-default": true,
  "crt-static-respected": true,
  "target-pointer-width": "64",
  "vendor": "unknown"
}
```

The engine reads the `OUS_TARGET` environment variable to select which target spec to use during automatic Rust builds. Set it to the desired triple before invoking the engine, or use `pipeline.sh` with `CUDANE_TARGETS` for multi-arch builds.

### 4. Complete Toolchain Agility

`Outsider` avoids hardcoded compilation workflows or locked execution loops. By processing commands via raw standard shell execution (`sh -c`), developers retain absolute toolchain freedom. You can easily build software across any cross-compilation landscape, architecture, or internal system structure simply by passing custom environment flags and wrappers directly into your manifest hooks.

</details>

<details><summary id="manualizing">Manualizing</summary>

## Linux from scratch

### Manual Archiving

To manually create an `.xcs` archive from a directory without going through the full build pipeline:

```shell
ous -a /path/to/staging/dir /path/to/output_xcs
```

This runs `tar` directly with Zstandard compression level 3.

### Manual Extraction

To extract an `.xcs` package (or multiple packages from a directory) into a target root:

```shell
ous -x /path/to/package.xcs /target/root
```

Or for all packages in a directory:

```shell
ous -x /path/to/packages/dir /target/root
```

The extractor first tries `tar` (native Zstd extraction), then falls back to `tar --zstd -xf` or `zstd -dc | tar -xf` for compatibility with tar+zstd archives.

Manual POSIX method:
```shell
for f in $HOME/path/to/*.xcs; do
    echo "Unpacking package: $(basename "$f")"
    tar --zstd -xf "$f" -C $HOME/path/to/rootfs/ 2>/dev/null || \
    zstd -dc "$f" | tar -xf - -C $HOME/path/to/rootfs/ 2>/dev/null || true
done
```

### Manual Metadata Generation

To generate a `metadata.json` for a directory without archiving it:

```shell
ous -w /path/to/source/dir /path/to/dest/dir
```

This creates a `metadata.json` in the destination directory with the package name derived from the source directory filename.

### Manual Checksum Typing (Optional)

You can choose a checksum type for your package:

```shell
ous -g <TYPE> <MANIFEST> <OUT>
```

supported types are: sha-256, sha-1, and md5 (Default: sha-256).

#### Defferences between Types

| Feature | MD5 (Message Digest 5) | SHA-1 (Secure Hash Algorithm 1) | SHA-256 (Secure Hash Algorithm 2) |
| --- | --- | --- | --- |
| **Output Size (Bits)** | 128 bits | 160 bits | 256 bits |
| **Output Size (Hex)** | 32 characters | 40 characters | 64 characters |
| **Security Status** | **Broken / Vulnerable** | **Broken / Vulnerable** | **Secure / Industry Standard** |
| **Collision Resistance** | Completely Broken | Cryptanalytically Broken | Extremely Strong |
| **Performance Speed** | Extremely Fast | Fast | Slower (More complex cycles) |

---

##### 1. Differences

###### Output Length & Bit Strength

* **MD5** produces a 128-bit hash value. Because of the shorter length, the total space of unique hashes is $2^{128}$, making it mathematically easier to find duplicates.
* **SHA-1** produces a 160-bit hash value ($2^{160}$ possibilities).
* **SHA-256** produces a 256-bit hash value ($2^{256}$ possibilities). This keyspace is astronomically massive, providing modern protection against brute-force attacks.

###### Security & Collision Resistance

A **collision** occurs when two different inputs produce the exact same output hash.

* **MD5:** Highly vulnerable to collisions. Attackers can generate fraudulent files with matching MD5 hashes in seconds on standard hardware.
* **SHA-1:** Shattered by Google in 2017 (the SHAttered attack), proving that practical collisions are achievable with sufficient cloud computing power.
* **SHA-256:** Currently collision-resistant. No practical or theoretical collision attacks have been successfully executed against it.

###### Speed and Computational Complexity

* **MD5** and **SHA-1** require fewer bitwise operations and rounds, making them computationally cheap and fast to execute.
* **SHA-256** uses a much more intensive mathematical design with 64 processing rounds, making it more CPU-intensive but far more resilient.

---

##### 2. Use Cases

###### MD5: Fast Data Integrity Checks (Non-Security)

> **Rule:** Never use MD5 for security, passwords, or digital signatures.

* **Checksums for Legacy Pipelines:** Used to verify that a file wasn't corrupted during transfer (e.g., checking if a download finished correctly over an unstable network).
* **Databases & Caching:** Used as a fast lookup key or hash map index where speed matters and security is irrelevant.

###### SHA-1: Legacy Compatibility & Git Content Tracking

> **Rule:** Deprecated for SSL/TLS certificates and modern cryptographic chains.

* **Git Version Control:** Git famously uses SHA-1 not as a security feature, but as a unique fingerprint mechanism to track commits, source trees, and blobs.
* **Legacy System Backwards Compatibility:** Maintaining links to older embedded infrastructure that does not support modern LLVM/Clang compilation features or newer primitives.

###### SHA-256: The Security and Production Standard

> **Rule:** The absolute baseline default for cryptographic infrastructure.

* **Package Management Metadata (e.g., Outsider/Cudane Linux):** Used to securely anchor manifest files (`metadata.json`) ensuring that packages have not been maliciously altered by a third party.
* **Password Hashing (with Salting):** Combined with stretching algorithms (like PBKDF2 or Argon2) to store secure authentication records.
* **SSL/TLS Certificates:** Used globally to secure HTTP traffic (HTTPS) across the internet.
* **Blockchain and Cryptocurrency:** The structural foundation for Bitcoin mining block validation and transaction integrity.

### Package Inspection

To inspect an `.xcs` package and view its metadata:

```shell
ous -i /path/to/package.xcs
```

This prints:

- The file path and size (in bytes and MB).
- The file type (from the `file` command).

</details>

<details><summary id=lifecycle">Lifecycle</summary>

1. **Validation**: `main.rs` reads the JSON manifest and deserializes it into a `Manifest` struct. If the JSON is malformed or missing required fields, an error is returned immediately.

2. **Short-Circuit Check**: `process()` checks if the output file `<name>-<version>.xcs` already exists in the output directory. If it does and `OUS_FORCE` is not set, the package is skipped entirely — no fetch, build, or archive operations occur.

3. **State Loading**: `process()` checks for `.ous/<package_name>/.state.json`. If found and `OUS_CLEAN` is not set, the state is loaded and completed steps are skipped. If `OUS_CLEAN` is set or no state file exists, the workspace is created fresh.

4. **Source Fetching** (conditional): If the `fetch` step is not marked complete in the state file, `fetch()` retrieves the source code into `src/` using the appropriate method (local copy, git clone, or curl+tar download). On success, the state file is updated.

5. **Build Execution** (conditional): If the `build` step is not marked complete, `build()` executes the build command in the `src/` directory. For Rust packages with empty `build_cmd`, automatic `cargo build --release` is triggered with Cudane-specific flags. The build log is persisted to `build_log.txt` for resume.

6. **Installation** (conditional): If the `install` step is not marked complete, `install()` copies built artifacts from `src/` to `pkg/`. For Rust packages with empty `install_cmd`, files from `target/release/` are automatically copied.

7. **Hashing** (conditional): If the `hash` step is not marked complete, `hash()` computes checksums (SHA-256, SHA-1, MD5) of the entire `pkg/` directory. Results are persisted to `checksums.json`.

8. **Dependency Resolution**: `scan()` performs the full dependency analysis:
    - **Build-log parsing** (`cdd()`): Extracts package names from configure/meson/cmake output.
    - **Library scanning** (`libdep()`): Reads ELF files and `.so` filenames to find needed libraries.
    - **Package resolution** (`mltp()`): Maps each library to the package that provides it, using the workspace's `pkg/` directories as a library index.
    - **Consolidation**: When 2+ libraries resolve to the same package, the dependency's `libraries` field is populated (see [Consolidation]).
    - **Transitive resolution** (`transitive()`): Walks the repository index to find transitive dependencies.

9. **Metadata Generation** (conditional): If the `metadata` step is not marked complete:
    - `mtd()` builds a `PackageMetadata` struct with license, checksums, dependencies, files, and provides.
    - `write()` writes `metadata.json` to the `pkg/` directory.
    - `index()` updates `index.json` in the output root.

10. **Archiving** (conditional): If the `archive` step is not marked complete, `archive()` compresses the `pkg/` directory into `<name>-<version>.xcs` using `tar` with Zstandard compression.

11. **State Finalization**: After archiving, all steps are marked complete in `.state.json`. The next invocation will find the `.xcs` and skip the entire pipeline.

</details>

<details><summary id="license">License</summary>

## The Unlicense

see [**`LICENSE`**](https://codeberg.org/Cudane/Outsider/src/branch/master/LICENSE) file for details.

</details>

<details><summary id="credits">Credits</summary>

[[**`Myden`**]](https://codeberg.org/myden): **`Cudane`**, **`MCX`** and **`Outsider`** Founder - Made with 🤍 and **Rust**.

</details>

---

`▐▀` `-` `▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▌`

- **`Version`:** **`0.5.0`**.
- **`Architecture`:** **`x86_64-pc-linux-musl`** (**`amd64`**).

`▐▄` `-` `▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▌`
