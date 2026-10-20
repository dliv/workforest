//! The forest's pure decisions: path newtypes, forest metadata types, the config file's format,
//! and the decisions, result types and renderers of the forest's commands. Nothing here reads the
//! filesystem, runs git or reads the clock.

pub mod config;
pub mod meta;
pub mod paths;
pub mod plan;
