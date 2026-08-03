{
  lib,
  craneLib,
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

let
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

  cargoSrc = craneLib.cleanCargoSource ./.;
  cargoExtraArgs =
    "--package skk-zenz-fcitx5" + lib.optionalString useVulkan " --features skk-zenz-backend/vulkan";

  commonArgs = {
    inherit cargoExtraArgs;
    src = cargoSrc;

    nativeBuildInputs = [
      clang
      cmake
      ninja
      pkg-config
    ]
    ++ lib.optionals useVulkan [
      shaderc
    ];

    buildInputs = [
      fcitx5
      libskk
      llvmPackages.openmp
    ]
    ++ lib.optionals useVulkan [
      vulkan-headers
      vulkan-loader
    ];

    env = {
      LIBCLANG_PATH = "${lib.getLib llvmPackages.libclang}/lib";
      SKK_RULES_PATH = "${libskk}/share/libskk/rules";
    };
  };

  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // {
      pname = "${pname}-deps";
      inherit version;
    }
  );
in
craneLib.mkCargoDerivation (
  commonArgs
  // {
    inherit
      pname
      version
      cargoArtifacts
      src
      ;
    doInstallCargoArtifacts = false;

    # CMake owns the final Cargo invocation and links its static library into the
    # Fcitx addon. crane restores cargoArtifacts before this phase runs.
    configurePhase = null;
    buildPhase = null;
    installPhase = null;

    cmakeFlags = [
      (lib.cmakeBool "SKK_ZENZ_VULKAN" useVulkan)
    ];

    buildPhaseCargoCommand = "cargo build --release ${cargoExtraArgs}";

    meta = {
      description = "SKK input method for Fcitx 5 with Zenzai reranking";
      homepage = "https://github.com/ibuki2003/skk_zenz";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
    };
  }
)
