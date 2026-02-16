{
  description = "nx - Nix configuration management tool";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        pkgs = nixpkgs.legacyPackages.${system};
      });
    in
    {
      packages = forAllSystems ({ pkgs }: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "nx";
          version = "0.1.0";

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

          # Only run unit tests in sandbox; integration tests need Python + ~/.nix-config
          cargoTestFlags = [ "--lib" ];

          meta = with pkgs.lib; {
            description = "Nix configuration management tool";
            license = licenses.mit;
            mainProgram = "nx";
          };
        };
      });
    };
}
