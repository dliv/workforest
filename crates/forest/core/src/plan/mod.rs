//! Pure planning: the types, decisions and renderers of the forest's commands. Nothing here
//! reads the filesystem, runs git or reads the clock; the forest ops crate's command modules
//! observe the world and execute.

pub mod branch_state;
pub mod ls;
pub mod new;
pub mod reset;
pub mod rm;
