# 11. Incremental Meta Writing

Date: 2026-02-15
Status: Accepted

## Context

`new` creates worktrees sequentially. If it fails on repo 3 of 5, the user needs `rm` to clean up repos 1–2. But `rm` reads `.forest-meta.toml` to know what exists. If meta is only written after all repos succeed, a partial failure leaves worktrees with no meta — orphaned state that `rm` cannot discover.

## Decision

`execute_plan()` in `src/commands/new.rs` writes `.forest-meta.toml` incrementally:

1. **Write initial meta** with header fields (`name`, `created_at`, `mode`) and an empty `repos` vec.
2. **After each successful worktree creation**, push the new `RepoMeta` onto `meta.repos` and rewrite the file.

"Incremental" means rewriting the full TOML with a growing repos vec each time — not append-only file operations. `write_meta()` (`src/forest.rs`) serializes the full struct via `toml::to_string_pretty`.

This pairs with `new`'s stop-on-failure error policy (ADR 0009). When a worktree cannot be created, execution stops and rolls back: it removes the worktrees this run added and deletes the branches it created that still point at the commit they were created at, then removes the forest directory. When the metadata itself cannot be written, execution returns without rollback, and the meta file on disk reflects the repos created before that write.

## Consequences

- **Partial forests stay valid:** A failed worktree creation rolls the forest back, so it leaves no partial forest; only a branch that gained commits during the run is kept, with those commits. A failed metadata write or an interrupted run can leave one; its meta lists the repos created before the failure, and `rm` reads that meta and cleans them up.
- **No orphaned state:** Every successfully created worktree is recorded in meta before the next one is attempted.
- **`rm` just works:** `plan_rm()` (`src/commands/rm.rs`) reads whatever repos are in meta and handles them — no special partial-forest logic needed. This depends on ADR 0004 (meta is self-contained).
- **Write amplification is negligible:** Rewriting a small TOML file (typically < 1KB) per repo is insignificant compared to the git operations surrounding it.
- If `new` is interrupted (kill signal), the meta may be one repo behind. Acceptable: the extra worktree is discoverable via `git worktree list` on the source repo.

Revisit if: `new` creates worktrees in parallel, or an interrupted run's unrecorded worktree is found to block `rm` or a rerun of `new`.
