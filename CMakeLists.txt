cmake_minimum_required(VERSION 3.21)
project(skk-zenz VERSION 0.1.0 LANGUAGES CXX)

include(GNUInstallDirs)
find_package(Fcitx5Core REQUIRED)
find_package(Fcitx5ModuleNotifications REQUIRED)
find_package(OpenMP REQUIRED)
include("${FCITX_INSTALL_CMAKECONFIG_DIR}/Fcitx5Utils/Fcitx5CompilerSettings.cmake")

find_program(CARGO_EXECUTABLE cargo REQUIRED)

option(SKK_ZENZ_VULKAN "Enable the Vulkan backend" OFF)

set(RUST_FEATURE_ARGS)
if(SKK_ZENZ_VULKAN)
    find_package(Vulkan REQUIRED)
    list(APPEND RUST_FEATURE_ARGS --features skk-zenz-backend/vulkan)
endif()

set(RUST_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target")
set(RUST_LIBRARY "${RUST_TARGET_DIR}/release/libskk_zenz_fcitx5.a")
file(
    GLOB_RECURSE RUST_SOURCES
    CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/fcitx5-addon/src/*.rs"
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/skk-zenz-backend/src/*.rs"
)

add_custom_command(
    OUTPUT "${RUST_LIBRARY}"
    COMMAND
        "${CMAKE_COMMAND}" -E env
        "CARGO_TARGET_DIR=${RUST_TARGET_DIR}"
        "${CARGO_EXECUTABLE}" build --release
        --package skk-zenz-fcitx5
        --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
        ${RUST_FEATURE_ARGS}
    DEPENDS
        "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
        "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.lock"
        "${CMAKE_CURRENT_SOURCE_DIR}/crates/fcitx5-addon/Cargo.toml"
        "${CMAKE_CURRENT_SOURCE_DIR}/crates/skk-zenz-backend/Cargo.toml"
        "${CMAKE_CURRENT_SOURCE_DIR}/crates/skk-zenz-backend/build.rs"
        ${RUST_SOURCES}
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    COMMENT "Building the Rust core"
    VERBATIM
)
add_custom_target(skk-zenz-rust DEPENDS "${RUST_LIBRARY}")

add_library(
    skk-zenz SHARED
    crates/fcitx5-addon/src/fcitx5_adapter.cpp
    "${RUST_LIBRARY}"
)
add_dependencies(skk-zenz skk-zenz-rust)
target_link_libraries(
    skk-zenz
    PRIVATE Fcitx5::Core Fcitx5::Module::Notifications OpenMP::OpenMP_CXX "${RUST_LIBRARY}"
)
if(SKK_ZENZ_VULKAN)
    target_link_libraries(skk-zenz PRIVATE Vulkan::Vulkan)
endif()
set_target_properties(skk-zenz PROPERTIES PREFIX "")

configure_file(
    data/skk-zenz-addon.conf.in
    "${CMAKE_CURRENT_BINARY_DIR}/skk-zenz-addon.conf"
)

install(TARGETS skk-zenz DESTINATION "lib/fcitx5")
install(
    FILES "${CMAKE_CURRENT_BINARY_DIR}/skk-zenz-addon.conf"
    RENAME skk-zenz.conf
    DESTINATION "share/fcitx5/addon"
)
install(
    FILES data/skk-zenz.conf
    DESTINATION "share/fcitx5/inputmethod"
)
