{
  description = "nx - Nix configuration management tool";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      nxVersion = "1.3.0";
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        inherit system;
        pkgs = nixpkgs.legacyPackages.${system};
      });
    in
    {
      packages = forAllSystems ({ pkgs }: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "nx";
          version = nxVersion;

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeCheckInputs = [ pkgs.git pkgs.which ];

          preCheck = ''
            export HOME="$TMPDIR"
            git config --global init.defaultBranch main
            git config --global user.email "test@test"
            git config --global user.name "test"
          '';

          # Only run unit tests in sandbox; integration tests require external repo/env setup.
          cargoTestFlags = [ "--lib" ];

          meta = with pkgs.lib; {
            description = "Nix configuration management tool";
            license = licenses.mit;
            mainProgram = "nx";
          };
        };
      });

      apps = forAllSystems ({ system, ... }: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/nx";
        };
      });
    };
}
