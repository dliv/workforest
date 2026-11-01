# Architecture policy tooling

Up: [Library crates](../README.md) · [Architecture rules](../../docs/architecture.md)

`git-forest-policy` is Tooling, not part of the product binary. Its tests read
product sources and manifests and the workspace graph from `cargo metadata`.
No production crate depends on it. [Architecture](../../docs/architecture.md)
owns the rules and their enforcement limits; this file maps their machinery.

| Area | Source and proof |
| --- | --- |
| File classes and allowed internal edges | [classification.rs](src/classification.rs); tests in [classification](tests/classification.rs) and [direction](tests/direction.rs) |
| Crate classes and dependency boundaries | [edges.rs](src/edges.rs), [metadata.rs](src/metadata.rs); [edge tests](tests/edges.rs) |
| Pure and Root source restrictions | [scan.rs](src/scan.rs), [tokens.rs](src/tokens.rs), [manifest.rs](src/manifest.rs); [ambient](tests/ambient.rs) and [scanner](tests/scanner.rs) tests |
| Declared traits | [traits.rs](tests/traits.rs), including its explicit allowlist |
| Planted violations | [fixtures](tests/fixtures/), used by the policy tests |
| Sanctioned Clippy sites | [control-expected.txt](control-expected.txt), checked by the [positive-control script](../../scripts/README.md) |

Run `just check` for the product and policy together. For a focused policy
test, `cargo test -p git-forest-policy --locked` runs this crate's suite, but
does not replace the workspace gate or Clippy positive control.

Follow [Changing the rules](../../docs/architecture.md#changing-the-rules)
when adding a module, crate, allowed dependency or exemption. Review the reason
and corresponding failure control together; do not update an expected count
just to make a changed warning disappear. The scanner catches ordinary drift,
not every spelling or effect hidden behind a dependency. The actual Pure-source check in `tests/ambient.rs` now calls arch-gates as a
pinned local Git dev-dependency. File selection and the other checks remain
local. The engine's Pure scanner fixtures run in its own `just check`; the
consumer gate explicitly invokes its checker. Root scanning and remaining
machinery are not migrated in this first slice. The local Git pin is a
machine-local checkpoint pending engine publication and portable qualification.
