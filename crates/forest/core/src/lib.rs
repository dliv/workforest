//! The forest's pure decisions: path newtypes, forest metadata types, and the plan, result and
//! renderer code of the planning commands. Nothing here reads the filesystem, runs git or reads
//! the clock.

pub mod meta;
pub mod paths;
pub mod plan;
