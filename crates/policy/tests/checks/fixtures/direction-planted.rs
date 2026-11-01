// Planted source for tests/direction.rs, scanned as if it were commands::rm.
// Never compiled. A mention of crate::version_check in a comment is not an edge.
use super::branch_state::X;
pub use crate::forest::Y;
use crate::paths::*;

pub(crate) fn a() {}

pub(super) fn b() {}

pub(in crate::commands) fn c() {}

fn reads(x: &str) {
    let _ = crate::meta::read(x);
    let _ = "crate::version_check is only text here";
}

#[cfg(test)]
mod tests {
    fn uses_git() {
        let _ = crate::git::git();
    }
}
