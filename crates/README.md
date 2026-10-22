# Library crates

Up: [Repository README](../README.md) · [Developer documentation](../docs/README.md)

The feature and edge libraries are reusable production units; the policy
crate checks them. The binary package remains at the repository root, with
its [composition code under `src/`](../src/README.md).

| Area | Responsibility |
| --- | --- |
| [forest](forest/README.md) | The feature: pure decisions in `core`, git/filesystem workflows in `ops` |
| [update-check](update-check/README.md) | The outbound update protocol and today's cache/background handshake |
| [policy](policy/README.md) | Tooling that checks classification, dependencies, effects and traits |

[ADR 0017](../docs/decisions/0017-crate-layout.md) owns the package layout
decision; [architecture](../docs/architecture.md#crate-classes-and-edges)
owns the allowed dependencies. Every member here is unpublished at version
`0.0.0`; the release version and channel features belong to the root package.

Before adding a feature, use the [placement test](../docs/candidate-features.md)
and [feature-start questions](../docs/design-principles.md#feature-start-questions).
Give the feature a README covering its responsibility and consumer, guarantees,
module roles, dependencies, proofs and upcoming work. The
[rule-change procedure](../docs/architecture.md#changing-the-rules) lists the
manual manifest and policy registrations; there is no feature generator today.
