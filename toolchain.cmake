cmake_minimum_required(VERSION 3.20)

set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSROOT /)

# ── Auto-detect host architecture ──────────────────────────────────────
execute_process(
  COMMAND uname -m
  OUTPUT_VARIABLE OUS_HOST_ARCH
  OUTPUT_STRIP_TRAILING_WHITESPACE
)

if(OUS_HOST_ARCH STREQUAL "x86_64")
  set(CMAKE_SYSTEM_PROCESSOR x86_64)
  set(OUS_CLANG_TARGET x86_64-unknown-linux-musl)
elseif(OUS_HOST_ARCH STREQUAL "aarch64")
  set(CMAKE_SYSTEM_PROCESSOR aarch64)
  set(OUS_CLANG_TARGET aarch64-unknown-linux-musl)
else()
  message(FATAL_ERROR "Unsupported architecture: ${OUS_HOST_ARCH}. Supported: x86_64, aarch64")
endif()

set(CMAKE_C_COMPILER clang)
set(CMAKE_CXX_COMPILER clang++)
set(CMAKE_C_FLAGS_INIT "-target ${OUS_CLANG_TARGET}")
set(CMAKE_CXX_FLAGS_INIT "-target ${OUS_CLANG_TARGET}")
