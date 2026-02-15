# 5. One Repo Type

Date: 2026-02-15
Status: Accepted

## Context

The original spec defined three repo types: `mutable` (active development), `branch-on-main` (personal repos branching off main), and `readonly` (coworker reference repos with shallow clones). This created a `type` field in config, type-specific branching logic, and shallow clone handling — complexity that didn't earn its keep. In practice, the only behavioral difference between repos is which branch they fork from.

## Decision

Consolidate to a single repo concept. Every repo is a git worktree. The only per-repo config knob is `base_branch`.

- `RepoConfig` (`src/config.rs`, lines 21–30) has no `type` field. Fields are `path`, `name`, `base_branch`, `remote`.
- `ResolvedRepo` (`src/config.rs`, lines 32–38) has `base_branch` as the only behavioral differentiator.
- `RepoMeta` (`src/meta.rs`, lines 31–38) has no type field — stores `source`, `branch`, `base_branch`, `branch_created`.
- `src/commands/new.rs` — all repos go through the same `CheckoutKind` resolution (ADR 0003). No type-based branching in any code path.

Whether you modify a repo is your runtime choice, not something encoded in config. Repos branching off `main` vs `dev` differ only in `base_branch`.

## Consequences

- **Simpler config:** No `type = "mutable"` field to learn or set. One concept to understand.
- **No shallow clones:** Everything is a worktree — no special git clone handling.
- **Original `readonly` use case still works:** A coworker's repo as a worktree on `forest/{name}` serves the same purpose without a dedicated type.
- **Simplifies ADR 0004:** Meta format has no type field to track or migrate.
- If a genuine need for type-specific behavior emerges (e.g., sparse checkout for large repos), it can be added as a new per-repo flag without redesigning the type system.
