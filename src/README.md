# Composition root and command line

Up: [Repository README](../README.md) · [Developer documentation](../docs/README.md)

This directory belongs to the root `git-forest` package, class Binary. The
files are Root or Pure; forest effects live in the [feature libraries](../crates/forest/README.md).

| Source | Responsibility |
| --- | --- |
| [bin/git-forest.rs](bin/git-forest.rs), [bin/git-forest-beta.rs](bin/git-forest-beta.rs) | Call the shared `main_entry` for the selected channel |
| [lib.rs](lib.rs) | Read process inputs, resolve file locations, dispatch operations, present results and choose exits |
| [cli.rs](cli.rs) | Pure argument declarations; parsing process arguments happens in the root |
| [channel.rs](channel.rs) | Pure compile-time names/channel and mutually exclusive stable/beta features |

[ADR 0017](../docs/decisions/0017-crate-layout.md) explains why the version,
channel and binary package remain here. The
[root composition contract](../docs/architecture.md#root-composition) owns
the dispatch rule: at most one command operation per arm, with no new business
predicates over forest, repository or config state.

The [update checker](../crates/update-check/README.md) currently owns its own
client and background spawn, and this host intercepts its internal argument.
That current handshake and the planned ownership change are distinguished in
the edge README and [planned follow-ups](../docs/architecture.md#planned-follow-ups).

[Process tests](../tests/README.md) prove the assembled command-line contract.
[Policy](../crates/policy/README.md) checks Root source restrictions and class
placement; it does not mechanically prove that dispatch contains no business
decisions. Review the searches named in the composition contract when changing
that boundary.
