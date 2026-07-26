{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    clang
    cmake
    llvmPackages.libclang
    libskk
    ninja
    pkg-config
    rustup
    shaderc
    vulkan-headers
    vulkan-loader
  ];

  shellHook = ''
    export CMAKE_PREFIX_PATH="${pkgs.fcitx5}''${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
    export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
    export SKK_RULES_PATH="${pkgs.libskk}/share/libskk/rules"
    export PKG_CONFIG_PATH="${pkgs.fcitx5}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  '';
}
