// Planted source for tests/traits.rs. Never compiled.

/// A doc comment that mentions trait Hidden does not declare it.
pub trait A<T>: Send {}

trait B {}

pub(crate) unsafe trait C {}

pub(in crate::deep) trait D {}

const TEXT: &str = "trait InString {}";

struct X;

impl std::fmt::Display for X {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "x")
    }
}

fn takes(_: &dyn A<u8>) {}

fn raw() {
    let r#trait = 1;
    let _ = r#trait;
}
