{
  description = "Rockhopper: a native GPUI launcher for Wayland";

  nixConfig = {
    extra-substituters = [ "https://korora-tech.cachix.org" ];
    extra-trusted-public-keys = [
      "korora-tech.cachix.org-1:42bs8TVW3uvxvMnuJtkXP5lkHFK7++Sral9uRDautuA="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # GPUI dlopens these at runtime; the dev shell needs them on the path.
      runtimeLibraryPath =
        pkgs:
        pkgs.lib.makeLibraryPath (
          with pkgs;
          [
            libxkbcommon
            vulkan-loader
            wayland
          ]
        );

      homeModule = import ./nix/home-manager.nix self;
    in
    {
      packages = forAllSystems (pkgs: rec {
        rockhopper = pkgs.callPackage ./nix/package.nix { };
        default = rockhopper;
      });

      # Builds Rockhopper with the consumer's nixpkgs, as `pkgs.rockhopper`.
      overlays.default = final: _prev: {
        rockhopper = final.callPackage ./nix/package.nix { };
      };

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = nixpkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.default;
          meta.description = "Open the Rockhopper launcher";
        };
      });

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = with pkgs; [
            cargo
            clippy
            rust-analyzer
            rustc
            rustfmt
          ];
          LD_LIBRARY_PATH = runtimeLibraryPath pkgs;
        };
      });

      homeModules.default = homeModule;
      # The older name for Home Manager modules, kept for existing configs.
      homeManagerModules.default = homeModule;

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
