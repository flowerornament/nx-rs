mod lint;
mod rebuild;
mod test;
mod undo;
mod update;
mod upgrade;

pub use lint::cmd_lint;
pub use rebuild::{cmd_rebuild, cmd_rebuild_with_command};
pub use test::cmd_test;
pub use undo::cmd_undo;
pub use update::cmd_update;
pub use upgrade::cmd_upgrade;

const DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";

#[cfg(test)]
mod tests;
