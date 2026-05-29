{ pkgs, src, version }:
pkgs.rustPlatform.buildRustPackage {
  pname = "nx";
  inherit version src;

  cargoDeps = pkgs.stdenvNoCC.mkDerivation {
    pname = "nx-cargo-vendor";
    inherit version src;

    nativeBuildInputs = [ pkgs.cargo pkgs.cacert ];

    buildPhase = ''
      runHook preBuild

      export HOME="$TMPDIR"
      export CARGO_HOME="$TMPDIR/cargo-home"
      export CARGO_HTTP_CAINFO="${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
      export CARGO_HTTP_USER_AGENT="cargo/${pkgs.cargo.version} nx-rs-nix-vendor"

      cargo vendor --locked --versioned-dirs "$out"
      cp Cargo.lock "$out/Cargo.lock"

      runHook postBuild
    '';

    dontConfigure = true;
    dontInstall = true;
    dontFixup = true;

    outputHash = "sha256-OyLHuOHvoeZIBTG7mYmzzsa48WaHNaw3SpJDft0dba8=";
    outputHashAlgo = "sha256";
    outputHashMode = "recursive";
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
