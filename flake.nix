{
  description = "SKK Zenz";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.crane.url = "github:ipetkov/crane";

  outputs =
    { nixpkgs, crane, ... }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [
        "x86_64-linux"
        "aarch64-linux"
      ];
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          craneLib = crane.mkLib pkgs;
        in
        rec {
          skkzenz-fcitx5 = pkgs.callPackage ./default.nix { inherit craneLib; };
          skkzenz-fcitx5-vulkan = skkzenz-fcitx5.override { useVulkan = true; };

          default = skkzenz-fcitx5;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = import ./shell.nix { inherit pkgs; };
        }
      );
    };
}
