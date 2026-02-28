# Pre-cutover Gate Bundle Archive

- Captured at: 2026-02-28 07:46:22 UTC
- Repo: /Users/morgan/code/nx-rs
- Commit at execution time: 5ed1779f908ba6143197f720d00a0ed704077a9f
- Target config: /Users/morgan/.nix-config

## Commands Executed

1. `just ci`
2. `just parity-check-rust`
3. `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`

## Results

- `just ci`: pass (exit 0) -> [01-just-ci.log](./01-just-ci.log)
- `just parity-check-rust`: pass (exit 0) -> [02-just-parity-check-rust.log](./02-just-parity-check-rust.log)
- `cutover-validate`: pass (exit 0) -> [03-py-nx-just-cutover-validate.log](./03-py-nx-just-cutover-validate.log)
- `cutover-validate` extracted report: [04-cutover-validation-report.md](./04-cutover-validation-report.md)

All required pre-cutover gates passed on target config.
