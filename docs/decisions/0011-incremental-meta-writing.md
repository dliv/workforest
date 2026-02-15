# 11. Incremental Meta Writing

Date: 2026-02-15
Status: Accepted

## Context

`new` creates worktrees sequentially. If it fails on repo 3 of 5, the user needs `rm` to clean up repos 1–2. But `rm` reads `.forest-meta.toml` to know what exists. If meta is only written after all repos succeed, a partial failure leaves worktrees with no meta — orphaned state that `rm` cannot discover.

## Decision

`execute_plan()` in `src/commands/new.rs` writes `.forest-meta.toml` incrementally:

1. **Write initial meta** with header fields (`name`, `created_at`, `mode`) and an empty `repos` vec (lines 312–320).
2. **After each successful worktree creation**, push the new `RepoMeta` onto `meta.repos` and rewrite the file (lines 363–371).

"Incremental" means rewriting the full TOML with a growing repos vec each time — not append-only file operations. `ForestMeta::write()` (`src/meta.rs`, lines 41–45) serializes the full struct via `toml::to_string_pretty`.

This pairs with `new`'s stop-on-failure error policy (ADR 0009): on failure, execution stops and the meta file reflects exactly the repos that were successfully created.

## Consequences

- **Partial forests are valid:** If `new` fails on repo 3, meta contains repos 1–2. `rm` reads that meta and cleans up those repos.
- **No orphaned state:** Every successfully created worktree is recorded in meta before the next one is attempted.
- **`rm` just works:** `plan_rm()` (`src/commands/rm.rs`) reads whatever repos are in meta and handles them — no special partial-forest logic needed. This depends on ADR 0004 (meta is self-contained).
- **Write amplification is negligible:** Rewriting a small TOML file (typically < 1KB) per repo is insignificant compared to the git operations surrounding it.
- If `new` is interrupted (kill signal), the meta may be one repo behind. Acceptable: the extra worktree is discoverable via `git worktree list` on the source repo.
