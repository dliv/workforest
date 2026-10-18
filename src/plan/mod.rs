//! Pure planning: the types, decisions and renderers of the planning commands. Nothing here
//! reads the filesystem, runs git or reads the clock; the command modules observe the world
//! and execute plans.

pub(crate) mod branch_state;
pub(crate) mod new;
pub(crate) mod rm;
