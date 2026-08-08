cmake_minimum_required(VERSION 3.20)
include(CheckIncludeFile)

get_filename_component(REPO_ROOT "${CMAKE_CURRENT_LIST_DIR}" ABSOLUTE)

set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSROOT /)
set(CMAKE_FIND_ROOT_PATH /system)

# ── Auto-detect host architecture ──────────────────────────────────────
execute_process(
  COMMAND uname -m
  OUTPUT_VARIABLE MCX_HOST_ARCH
  OUTPUT_STRIP_TRAILING_WHITESPACE
)

if(MCX_HOST_ARCH STREQUAL "x86_64")
  set(CMAKE_SYSTEM_PROCESSOR x86_64)
  set(MCX_CLANG_TARGET x86_64-unknown-linux-musl)
elseif(MCX_HOST_ARCH STREQUAL "aarch64")
  set(CMAKE_SYSTEM_PROCESSOR aarch64)
  set(MCX_CLANG_TARGET aarch64-unknown-linux-musl)
else()
  message(FATAL_ERROR "Unsupported architecture: ${MCX_HOST_ARCH}. Supported: x86_64, aarch64")
endif()

set(CMAKE_C_COMPILER clang)
set(CMAKE_CXX_COMPILER clang++)
set(CMAKE_C_FLAGS_INIT "-target ${MCX_CLANG_TARGET}")
set(CMAKE_CXX_FLAGS_INIT "-target ${MCX_CLANG_TARGET}")

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
