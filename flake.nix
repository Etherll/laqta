{
  description = "Laqta: capture, record, mark up and share your screen";

  # The CI build (.github/workflows/nix.yml) is pushed here. Nix asks before using it, and uses it
  # only for trusted users; a NixOS system adds it in `nix.settings` instead (see the README).
  nixConfig = {
    extra-substituters = [ "https://laqta.cachix.org" ];
    extra-trusted-public-keys = [ "laqta.cachix.org-1:ZTaWCeegQ7a7vo/SbzIcA/CCGIVl+Syry34569o9Rz4=" ];
  };

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        laqta = pkgs.callPackage ./nix/package.nix { };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.laqta;
      });

      overlays.default = final: _prev: { laqta = final.callPackage ./nix/package.nix { }; };

      devShells = forAllSystems (
        pkgs:
        let
          laqta = self.packages.${pkgs.stdenv.hostPlatform.system}.laqta;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ laqta ];
            packages = [
              pkgs.clippy
              pkgs.rust-analyzer
              pkgs.rustfmt
            ];
            # The package's build-time paths (nix/package.nix), so `cargo run` behaves like it.
            inherit (laqta) LAQTA_TOOLS_PATH LAQTA_ONNXRUNTIME;
            # `cargo run` binaries have no patched RUNPATH, so the dlopen'd libraries come from here.
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath laqta.buildInputs;
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
