use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

pub fn resolve_nx_bin(workspace_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = env::var_os("CARGO_BIN_EXE_nx") {
        return Ok(PathBuf::from(path));
    }

    let candidate = workspace_root.join("target/debug/nx");
    if candidate.exists() {
        return Ok(candidate);
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "missing nx test binary").into())
}
