#!/bin/bash
export PKG_CONFIG_SYSROOT_DIR="/"
export PKG_CONFIG_LIBDIR="/system/lib/pkgconfig"
exec pkg-config "$@"
