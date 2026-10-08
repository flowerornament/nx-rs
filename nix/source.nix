{ lib }:
lib.fileset.toSource {
  root = ../.;
  fileset = lib.fileset.unions [
    ../Cargo.toml
    ../Cargo.lock
    ../src
    ../tests
    ../.cargo
    # Compiled into the package's unit tests.
    ../README.md
    ../.agents/SPEC.md
  ];
}
