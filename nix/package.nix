{ pkgs, src, version }:
pkgs.rustPlatform.buildRustPackage {
  pname = "nx";
  inherit version src;

  cargoLock = {
    lockFile = src + /Cargo.lock;
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
}
