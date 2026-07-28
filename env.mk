REPO_ROOT   := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
SYSROOT     := $(REPO_ROOT)/rootfs
PREFIX      ?= /system

# ── Auto-detect host architecture ──────────────────────────────────────
HOST_ARCH_RAW := $(shell uname -m)
ifeq ($(HOST_ARCH_RAW),x86_64)
  ARCH         := amd64
  RUST_TARGET  := x86_64-unknown-linux-musl
  GCC_TARGET   := x86_64-unknown-linux-musl
  CMAKE_ARCH   := x86_64
  MESON_CPU    := x86_64
else ifeq ($(HOST_ARCH_RAW),aarch64)
  ARCH         := arm64
  RUST_TARGET  := aarch64-unknown-linux-musl
  GCC_TARGET   := aarch64-unknown-linux-musl
  CMAKE_ARCH   := aarch64
  MESON_CPU    := aarch64
else
  $(error Unsupported architecture: $(HOST_ARCH_RAW). Supported: x86_64, aarch64)
endif

CC            := clang --target=$(GCC_TARGET) --sysroot=$(SYSROOT)
CXX           := clang++ --target=$(GCC_TARGET) --sysroot=$(SYSROOT)
AR            := llvm-ar
STRIP         := llvm-strip

CFLAGS        := -O2 -nostdinc -isystem $(SYSROOT)$(PREFIX)/include
CXXFLAGS      := -O2 -nostdinc++ -isystem $(SYSROOT)$(PREFIX)/include
LDFLAGS       := -L$(SYSROOT)$(PREFIX)/lib -Wl,-rpath,$(PREFIX)/lib

export PKG_CONFIG_SYSROOT_DIR := $(SYSROOT)
export PKG_CONFIG_LIBDIR      := $(SYSROOT)$(PREFIX)/lib/pkgconfig:$(SYSROOT)$(PREFIX)/share/pkgconfig
export PKG_CONFIG_PATH        :=
