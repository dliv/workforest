# 4. Forest Meta Is Self-Contained

Date: 2026-02-15
Status: Accepted

## Context

Commands that operate on existing forests (`rm`, `status`, `exec`, `ls`) need repo metadata: source paths, branch names, base branches. This data could come from global config (re-resolving at runtime) or from the forest's own meta file (snapshot at creation time). Re-resolving from config couples post-creation commands to config state and means editing config retroactively changes existing forests — a surprising side effect.

## Decision

`.forest-meta.toml` captures all resolved values at creation time. Post-creation commands use only the meta file, never the global config.

`ForestMeta` and `RepoMeta` (`src/meta.rs`, lines 23–38) store everything needed: `name`, `source` (absolute path), `branch`, `base_branch`, `branch_created` per repo.

Commands depend only on meta + forest directory:

- `plan_rm(forest_dir, meta)` (`src/commands/rm.rs`, line 53) — no config parameter.
- `cmd_status(forest_dir, meta)` (`src/commands/status.rs`, line 27) — no config parameter.
- `cmd_exec(forest_dir, meta, cmd)` (`src/commands/exec.rs`, line 13) — no config parameter.

Config is loaded in `main.rs` only to resolve `worktree_base` for forest discovery, then meta takes over.

## Consequences

- **Config changes don't affect existing forests.** Changing `base_branch` in config has no effect on forests already created.
- **`rm` is self-sufficient.** It has source paths, branch names, and `branch_created` flags — everything needed for cleanup without consulting config.
- **No config migration concerns.** Each forest is a snapshot of creation-time state. Config schema can evolve freely.
- **Config is only used by `init` (writes it) and `new` (reads it for defaults).** All other commands are config-independent.
- Reinforced by ADR 0005 (no type field to track) and ADR 0012 (no template name in meta). Meta stores resolved `base_branch` per repo, enabling ADR 0011 (incremental writes make partial forests self-describing).
