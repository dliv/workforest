//! Architecture policy checks for git-forest.
//!
//! This crate is test-only: it never builds or runs the product. Its tests read
//! the product's sources under `src/` and its manifests, and the workspace graph
//! from `cargo metadata`, and fail when they break a mechanically checkable
//! architecture rule. Its one dependency, `serde_json`, parses that graph. The
//! rules and their homes are documented in `docs/architecture.md`.

use std::path::{Path, PathBuf};

pub mod classification;
pub mod edges;
pub mod manifest;
pub mod metadata;
pub mod scan;
mod tokens;

/// The repository root: the directory holding the root `Cargo.toml` and `src/`.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the policy crate lives two levels below the repository root")
}
