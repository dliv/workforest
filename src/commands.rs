use anyhow::Result;
use std::path::Path;

use crate::meta::ForestMeta;

pub fn cmd_ls(worktree_base: &Path) -> Result<()> {
    let _ = worktree_base;
    eprintln!("ls: not yet implemented");
    Ok(())
}

pub fn cmd_status(forest_dir: &Path, meta: &ForestMeta) -> Result<()> {
    let _ = (forest_dir, meta);
    eprintln!("status: not yet implemented");
    Ok(())
}

pub fn cmd_exec(forest_dir: &Path, meta: &ForestMeta, cmd: &[String]) -> Result<()> {
    let _ = (forest_dir, meta, cmd);
    eprintln!("exec: not yet implemented");
    Ok(())
}
