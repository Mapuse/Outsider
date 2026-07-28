#!/bin/bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export PKG_CONFIG_SYSROOT_DIR="${SCRIPT_DIR}/rootfs"
export PKG_CONFIG_LIBDIR="${SCRIPT_DIR}/rootfs/system/lib/pkgconfig"
exec pkg-config "$@"
