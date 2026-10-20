//! The forest's effects: each command's observe and execute steps, the git wrapper, forest
//! discovery and metadata I/O, and the config file's I/O. Decisions live in `git-forest-core`;
//! the composition root reads the environment and the working directory, resolves the file
//! locations and passes them in.
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        reason = "unit tests build real git repositories; the non-test build still lints production code"
    )
)]

pub mod commands;
pub mod config;
pub mod forest;
mod git;
mod testutil;
