REPO_ROOT   := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
SYSROOT     := /
PREFIX      ?= /system

# ── cps revision (single source of truth) ─────────────────────────────
# Every builder must pin the same cps commit: Cargo.toml (dependency rev),
# Makefile ($(CPS_REV)), build.ninja / meson.build / CMakeLists.txt
# (${CPS_REV:-<literal>} fallbacks). Update ALL of them together.
CPS_REV ?= c4ba21e185398558052acec3f0b4619b4e8c0678
export CPS_REV

# ── Auto-detect host architecture ──────────────────────────────────────
HOST_ARCH_RAW := $(shell uname -m)
ifeq ($(HOST_ARCH_RAW),x86_64)
  ARCH         := amd64
  RUST_TARGET  := x86_64-unknown-linux-musl
  CLANG_TARGET := x86_64-unknown-linux-musl
  CMAKE_ARCH   := x86_64
  MESON_CPU    := x86_64
else ifeq ($(HOST_ARCH_RAW),aarch64)
  ARCH         := arm64
  RUST_TARGET  := aarch64-unknown-linux-musl
  CLANG_TARGET := aarch64-unknown-linux-musl
  CMAKE_ARCH   := aarch64
  MESON_CPU    := aarch64
else
  $(error Unsupported architecture: $(HOST_ARCH_RAW). Supported: x86_64, aarch64)
endif

# Cross toolchain defaults. The ous build itself is pure cargo (RUST_TARGET
# below); CC/CFLAGS/AR/... exist for manifest authors and C-based packages
# built *through* ous, so a native build and an `ous`-produced package see
# the same compiler settings.
CC            := clang --target=$(CLANG_TARGET) --sysroot=$(SYSROOT)
CXX           := clang++ --target=$(CLANG_TARGET) --sysroot=$(SYSROOT)
AR            := llvm-ar
STRIP         := llvm-strip

CFLAGS        := -O2 -nostdinc -isystem $(SYSROOT)$(PREFIX)/include
CXXFLAGS      := -O2 -nostdinc++ -isystem $(SYSROOT)$(PREFIX)/include
LDFLAGS       := -L$(SYSROOT)$(PREFIX)/lib -Wl,-rpath,$(PREFIX)/lib

export PKG_CONFIG_SYSROOT_DIR := $(SYSROOT)
export PKG_CONFIG_LIBDIR      := $(SYSROOT)$(PREFIX)/lib/pkgconfig:$(SYSROOT)$(PREFIX)/share/pkgconfig
export PKG_CONFIG_PATH        :=
