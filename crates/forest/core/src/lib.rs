//! The forest's pure decisions: path newtypes, forest metadata types, and the decisions, result
//! types and renderers of the forest's commands. Nothing here reads the filesystem, runs git or
//! reads the clock.

pub mod meta;
pub mod paths;
pub mod plan;
