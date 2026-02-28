# nx-rs Maintenance Gate Report

- Executed (UTC): 2026-02-28 14:13:51 UTC
- Cadence: weekly
- Workspace root: /Users/morgan/code/nx-rs
- Report directory: /Users/morgan/code/nx-rs/.agents/reports/maintenance-gates/20260228T141351Z
- Failures: 0

## Steps

| Step | Command | Exit | Status | Log |
| --- | --- | --- | --- | --- |
| 01-just-ci | `just ci` | 0 | pass | `01-just-ci.log` |
| 02-parity-check-rust | `just parity-check-rust` | 0 | pass | `02-parity-check-rust.log` |
| 03-cutover-validate | `PY_NX=/Users/morgan/code/nx-python/nx just cutover-validate` | 0 | pass | `03-cutover-validate.log` |

## Overall Gate

- Result: pass
