{ pkgs, src, version }:
pkgs.rustPlatform.buildRustPackage {
  pname = "nx";
  inherit version src;

  cargoLock = {
    lockFile = src + "/Cargo.lock";
    # Avoid crates.io API fetches; Cargo lockfile checksums still verify content.
    extraRegistries = {
      "https://github.com/rust-lang/crates.io-index" = "https://static.crates.io/crates";
    };
  };

  configurePhase = ''
    runHook preConfigure

    # importCargoLock emits an extra source alias for extraRegistries. When the
    # alias is crates.io's git index, Cargo treats it as a duplicate registry.
    for config in .cargo/config.toml ../.cargo/config.toml; do
      if [ -f "$config" ]; then
        awk '
          /^[[:space:]]*\[source\."https:\/\/github.com\/rust-lang\/crates.io-index"\][[:space:]]*$/ { skip = 2; next }
          skip > 0 { skip--; next }
          { print }
        ' "$config" > "$config.tmp"
        mv "$config.tmp" "$config"
      fi
    done

    runHook postConfigure
  '';

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
