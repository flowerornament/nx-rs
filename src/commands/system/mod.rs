mod rebuild;
mod test_cmd;
mod undo;
mod update;
mod upgrade;

pub use rebuild::cmd_rebuild;
pub use test_cmd::cmd_test;
pub use undo::cmd_undo;
pub use update::cmd_update;
pub use upgrade::cmd_upgrade;

const DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";

#[cfg(test)]
mod tests;
