#!/bin/sh
# gen-cross.sh — Generate Meson cross-file for Cudane with auto-detection
# Usage: ./gen-cross.sh [output_path]
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-${SCRIPT_DIR}/cross.txt}"
SYSROOT="${SCRIPT_DIR}/rootfs"

# ── Auto-detect architecture ───────────────────────────────────────────
HOST_ARCH=$(uname -m)
case "$HOST_ARCH" in
  x86_64)
    GCC_TARGET="x86_64-unknown-linux-musl"
    MESON_CPU="x86_64"
    ;;
  aarch64)
    GCC_TARGET="aarch64-unknown-linux-musl"
    MESON_CPU="aarch64"
    ;;
  *)
    echo "error: unsupported architecture: $HOST_ARCH (supported: x86_64, aarch64)" >&2
    exit 1
    ;;
esac

cat > "$OUT" <<EOF
[binaries]
c = 'clang'
cpp = 'clang++'
ar = 'llvm-ar'
strip = 'llvm-strip'
pkg-config = '${SCRIPT_DIR}/pkgconfig.sh'

[properties]
sys_root = '${SYSROOT}'

[built-in options]
c_args = ['-target', '${GCC_TARGET}', '-O2', '-Wno-undef', '-stdlib=libc++', '-I${SYSROOT}/system/include', '-I${SYSROOT}/system/include/libxml2', '-mllvm', '-polly', '-mllvm', '-polly-ast-use-expr-compiler', '-mllvm', '-polly-vectorizer=stripmine', '-flto']
cpp_args = ['-O3', '-target', '${GCC_TARGET}', '-Wno-undef', '-stdlib=libc++', '-mllvm', '-polly', '-mllvm', '-polly-ast-use-expr-compiler', '-mllvm', '-polly-vectorizer=stripmine', '-flto', '-I${SYSROOT}/system/include', '-I${SYSROOT}/system/include/libxml2']
c_link_args = ['-target', '${GCC_TARGET}', '-L${SYSROOT}/system/lib']
cpp_link_args = ['-target', '${GCC_TARGET}', '-L${SYSROOT}/system/lib']

[host_machine]
system = 'linux'
cpu_family = '${MESON_CPU}'
cpu = '${MESON_CPU}'
endian = 'little'
EOF

echo "cross.txt generated for ${MESON_CPU} → ${OUT}"
