{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    clang
    cmake
    llvmPackages.libclang
    ninja
    pkg-config
    rustup
  ];

  shellHook = ''
    export CMAKE_PREFIX_PATH="${pkgs.fcitx5}''${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
    export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
    export PKG_CONFIG_PATH="${pkgs.fcitx5}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  '';
}
