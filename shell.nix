{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    cargo
    cmake
    ninja
    pkg-config
    rustc
  ];

  shellHook = ''
    export CMAKE_PREFIX_PATH="${pkgs.fcitx5}''${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
    export PKG_CONFIG_PATH="${pkgs.fcitx5}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  '';
}
