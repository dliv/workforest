# Architecture policy tooling

Up: [Library crates](../README.md) · [Architecture rules](../../docs/architecture.md)

This Tooling crate is never part of the product binary. Normal-library
[src/metadata.rs](src/metadata.rs) owns the inventoried Cargo subprocess and
returns raw JSON for an explicit manifest/root. [src/lib.rs](src/lib.rs) locates
this consumer. It imports no engine types.

One integration target, [tests/policy.rs](tests/policy.rs), composes the checks.
[Support](tests/support/mod.rs) reads manifests and calls shared metadata parsing;
[classification](tests/support/classification.rs) owns file classes, Pure lists
and module exceptions; [package policy](tests/support/edges.rs) owns package
classes, external lists, feature layout, direction permissions and reasons.

The shared engine is a pinned Git dev-dependency. `arch_gates::metadata`,
`manifest` and `packages` parse supplied facts and evaluate dependencies/features;
`graph` and `scan` evaluate coverage/direction, Pure/Root effects, external
references and trait discovery. The engine performs no consumer observation.
The real workspace and planted controls in [checks](tests/checks/) remain in
normal `just check`; all previous 39 tests run. The duplicate normal-library
scanner/token code is removed. Workforest's Root permissions, external lists,
trait acceptance and diagnostics stay in local integration support/checks.

[Native consistency](tests/checks/native.rs) adds the fortieth policy test: it
checks Workforest's own method/print/debug baseline and pairs Root environment
permissions with native bans. TOML parsing is a policy dev-dependency only.

The [native control inventory](control-expected.txt) and shell adapter stay local.
The Cargo spawn is still inventoried at its original source file, with the
same compiler expectation. Engine-typed data never enters this normal library.

Run `just check` at the product root. A focused policy run is
`cargo test -p git-forest-policy --locked`; it does not replace the complete gate.
Classification, allowances and declared Pure features are reviewed local policy;
the shared evaluator rejects stale lists, missing lint inheritance and forbidden
Tooling dependencies, including external arch-gates aliases across dependency
kinds. See [architecture](../../docs/architecture.md) for enforcement limits.

Consumer pins currently name an immutable local file Git revision. This is a
machine-local checkpoint pending engine publication and independent portable
qualification. No remote or clean-machine CI success is claimed.
