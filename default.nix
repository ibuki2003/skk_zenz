{
  lib,
  stdenv,
  rustPlatform,
  cargo,
  rustc,
  cmake,
  ninja,
  pkg-config,
  clang,
  llvmPackages,
  fcitx5,
  libskk,
  shaderc,
  vulkan-headers,
  vulkan-loader,
  useVulkan ? false,
}:

stdenv.mkDerivation (finalAttrs: {
  pname = "skk-zenz";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.lock
      ./Cargo.toml
      ./CMakeLists.txt
      ./crates
      ./data
    ];
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit (finalAttrs) src;
    hash = "sha256-ceQQBLKwsUUl8tYQpDxHu3q4vaQ1W+rJvLxIEfKStH8=";
  };

  nativeBuildInputs = [
    cargo
    clang
    cmake
    ninja
    pkg-config
    rustPlatform.cargoSetupHook
    rustc
  ] ++ lib.optionals useVulkan [
    shaderc
  ];

  buildInputs = [
    fcitx5
    libskk
    llvmPackages.openmp
  ] ++ lib.optionals useVulkan [
    vulkan-headers
    vulkan-loader
  ];

  cmakeFlags = [
    (lib.cmakeBool "SKK_ZENZ_VULKAN" useVulkan)
  ];

  env = {
    LIBCLANG_PATH = "${lib.getLib llvmPackages.libclang}/lib";
    SKK_RULES_PATH = "${libskk}/share/libskk/rules";
  };

  meta = {
    description = "SKK input method for Fcitx 5 with Zenzai reranking";
    homepage = "https://github.com/ibuki2003/skk_zenz";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
})
