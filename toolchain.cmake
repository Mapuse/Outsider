cmake_minimum_required(VERSION 3.20)
include(CheckIncludeFile)

set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSROOT /home/m/cudane-build/rootfs)
set(CMAKE_FIND_ROOT_PATH /home/m/cudane-build/rootfs/system)

# ── Auto-detect host architecture ──────────────────────────────────────
execute_process(
  COMMAND uname -m
  OUTPUT_VARIABLE CUDANE_HOST_ARCH
  OUTPUT_STRIP_TRAILING_WHITESPACE
)

if(CUDANE_HOST_ARCH STREQUAL "x86_64")
  set(CMAKE_SYSTEM_PROCESSOR x86_64)
  set(CUDANE_GCC_TARGET x86_64-pc-linux-musl)
elseif(CUDANE_HOST_ARCH STREQUAL "aarch64")
  set(CMAKE_SYSTEM_PROCESSOR aarch64)
  set(CUDANE_GCC_TARGET aarch64-pc-linux-musl)
else()
  message(FATAL_ERROR "Unsupported architecture: ${CUDANE_HOST_ARCH}. Supported: x86_64, aarch64")
endif()

set(CMAKE_C_COMPILER clang)
set(CMAKE_CXX_COMPILER clang++)
set(CMAKE_C_FLAGS_INIT "-target ${CUDANE_GCC_TARGET}")
set(CMAKE_CXX_FLAGS_INIT "-target ${CUDANE_GCC_TARGET}")

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
