{
  description = "nx - Nix configuration management tool";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      nxVersion = "1.5.30";
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        inherit system;
        pkgs = nixpkgs.legacyPackages.${system};
      });
    in
    {
      packages = forAllSystems ({ pkgs, ... }: {
        default = import ./nix/package.nix {
          inherit pkgs;
          src = ./.;
          version = nxVersion;
        };
      });

      apps = forAllSystems ({ system, ... }: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/nx";
        };
      });

      homeManagerModules.default = import ./nix/home-manager.nix {
        src = ./.;
        inherit nxVersion;
      };
    };
}
