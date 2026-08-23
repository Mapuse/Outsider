cmake_minimum_required(VERSION 3.20)

# Cross-compilation defaults must be in effect before project() runs, and
# users may also pass this file via -DCMAKE_TOOLCHAIN_FILE (double-include
# is harmless — everything it sets is idempotent).
include("${CMAKE_CURRENT_LIST_DIR}/toolchain.cmake")

project(ous NONE)

include(ExternalProject)

# Install layout default: /system, matching the other builders.
if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
  set(CMAKE_INSTALL_PREFIX /system CACHE PATH "Install prefix" FORCE)
endif()

set(OUS_PROFILE release CACHE STRING "Cargo profile used to build ous (release|dev)")
set(OUS_CARGO_TARGET_DIR "${CMAKE_BINARY_DIR}/cargo")

# Auto-detect architecture when toolchain.cmake was not included above.
if(NOT DEFINED OUS_RUST_TARGET)
  execute_process(COMMAND uname -m OUTPUT_VARIABLE OUS_ARCH OUTPUT_STRIP_TRAILING_WHITESPACE)
  if(OUS_ARCH STREQUAL "x86_64")
    set(OUS_RUST_TARGET x86_64-unknown-linux-musl)
  elseif(OUS_ARCH STREQUAL "aarch64")
    set(OUS_RUST_TARGET aarch64-unknown-linux-musl)
  else()
    message(FATAL_ERROR "Unsupported architecture: ${OUS_ARCH}. Supported: x86_64, aarch64")
  endif()
endif()

set(OUS_BINARY "${OUS_CARGO_TARGET_DIR}/${OUS_RUST_TARGET}/${OUS_PROFILE}/ous")
if(OUS_PROFILE STREQUAL dev)
  set(OUS_BINARY "${OUS_CARGO_TARGET_DIR}/${OUS_RUST_TARGET}/debug/ous")
endif()

ExternalProject_Add(ous_build
  SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}"
  BUILD_IN_SOURCE TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND env CARGO_TARGET_DIR=${OUS_CARGO_TARGET_DIR}
    cargo build --profile ${OUS_PROFILE} --locked --target ${OUS_RUST_TARGET}
  BUILD_ALWAYS TRUE
  INSTALL_COMMAND ""
  BUILD_BYPRODUCTS "${OUS_BINARY}"
)

install(FILES "${OUS_BINARY}"
  DESTINATION bin
  PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE
)
install(FILES docs/ous.1
  DESTINATION share/man/man1
)

# Ship the project's own themes alongside cps data from the pinned clone.
install(DIRECTORY themes/
  DESTINATION share/cps/themes
  FILES_MATCHING PATTERN "*.py"
)

# ── cps: external host-side package, fetched from upstream ────────────────
# OUS_CPS_REV must stay in sync with CPS_REV in env.mk / Cargo.toml /
# build.ninja / meson.build.
set(OUS_CPS_URL https://github.com/Mapuse/CPS)
set(OUS_CPS_REV c4ba21e185398558052acec3f0b4619b4e8c0678 CACHE STRING "cps commit to build")
set(OUS_CPS_DIR "${CMAKE_BINARY_DIR}/cps")

ExternalProject_Add(cps_build
  GIT_REPOSITORY "${OUS_CPS_URL}"
  GIT_TAG "${OUS_CPS_REV}"
  GIT_SHALLOW TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND cargo build --release --locked
  BUILD_IN_SOURCE TRUE
  INSTALL_COMMAND ""
  BUILD_BYPRODUCTS "${OUS_CPS_DIR}/target/release/cps"
  EXCLUDE_FROM_ALL TRUE
)

install(FILES "${OUS_CPS_DIR}/target/release/cps"
  DESTINATION bin
  PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE
)
install(FILES "${OUS_CPS_DIR}/t.desc" "${OUS_CPS_DIR}/p.desc"
  DESTINATION share/cps
  OPTIONAL
)
if(EXISTS "${OUS_CPS_DIR}/examples")
  install(DIRECTORY "${OUS_CPS_DIR}/examples"
    DESTINATION share/cps
    FILES_MATCHING PATTERN "*.py"
  )
endif()
