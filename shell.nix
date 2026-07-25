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
  ];

  shellHook = ''
    export CMAKE_PREFIX_PATH="${pkgs.fcitx5}''${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
    export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
    export LIBSKK_DATA_PATH="''${XDG_CONFIG_HOME:-$HOME/.config}/libskk:${pkgs.libskk}/share/libskk''${LIBSKK_DATA_PATH:+:$LIBSKK_DATA_PATH}"
    export PKG_CONFIG_PATH="${pkgs.fcitx5}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  '';
}
