{
  description = "Gaze: Linux facial authentication (daemon, CLI, GUI, PAM, and GNOME Shell extension)";

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
    in
    {
      overlays.default = final: prev: {
        gaze = final.callPackage ./packaging/nix/gaze.nix { };
        gaze-gui = final.callPackage ./packaging/nix/gaze-gui.nix { };
        gaze-gnome-extension = final.callPackage ./packaging/nix/gnome-shell-extension.nix { };
      };

      packages = forAllSystems (pkgs: rec {
        gaze = pkgs.callPackage ./packaging/nix/gaze.nix { };
        gaze-gui = pkgs.callPackage ./packaging/nix/gaze-gui.nix { };
        gaze-gnome-extension = pkgs.callPackage ./packaging/nix/gnome-shell-extension.nix { };
        default = gaze;
      });

      nixosModules = rec {
        gaze =
          { pkgs, lib, ... }:
          let
            packages = self.packages.${pkgs.stdenv.hostPlatform.system};
          in
          {
            imports = [ ./packaging/nix/nixos-module.nix ];
            services.gaze = {
              package = lib.mkDefault packages.gaze;
              gui.package = lib.mkDefault packages.gaze-gui;
              gnome.extensionPackage = lib.mkDefault packages.gaze-gnome-extension;
            };
          };
        default = gaze;
      };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [
            self.packages.${pkgs.stdenv.hostPlatform.system}.gaze
            self.packages.${pkgs.stdenv.hostPlatform.system}.gaze-gui
          ];
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            just
          ];
          env = {
            ORT_STRATEGY = "system";
            ORT_LIB_LOCATION = "${pkgs.lib.getLib pkgs.onnxruntime}/lib";
            ORT_PREFER_DYNAMIC_LINK = "1";
          };
        };
      });

      checks = forAllSystems (pkgs: {
        inherit (self.packages.${pkgs.stdenv.hostPlatform.system})
          gaze
          gaze-gui
          gaze-gnome-extension
          ;
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
