/// Result structs for command output. Commands return these instead of printing
/// directly — main.rs formats them as human-readable or JSON based on --json.
/// See architecture-decisions.md, Decision 8.
mod exec;
mod init;
mod ls;
mod new;
mod rm;
mod status;

pub use exec::*;
pub use init::*;
pub use ls::*;
pub use new::*;
pub use rm::*;
pub use status::*;
