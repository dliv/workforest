# Feature Request: `git forest rm --all` + base-branch collision guard

## Problem 1: No bulk remove that preserves config

| Command                                    | Removes forests | Preserves config                     |
| ------------------------------------------ | --------------- | ------------------------------------ |
| `git forest rm <name>`                     | One at a time   | Yes                                  |
| `git forest reset --confirm`               | All             | No                                   |
| `git forest reset --config-only --confirm` | None            | No (deletes config, keeps worktrees) |

The gap: **remove all forests in one operation, keep config and templates intact.**

### How I hit this

Two stale forests where worktree directories existed on disk but git no longer recognized them as worktrees. `git forest rm <name> --force` failed for each because `git worktree remove` errored on stale references. With no `--all` flag, the only way forward was `git forest reset --confirm`, which also destroyed template configuration.

### Solution: `git forest rm --all [--force] [--dry-run] [--json]`

Iterates every forest in state and removes worktrees, branches, and forest directories. Leaves `config.toml` untouched. Partial failures: successful forests are removed from state, failures are reported, exit non-zero.

## Problem 2: Forest branch can collide with base branch

An agent created a forest with branch name `dev` on a repo where `dev` was the base branch, blocking work outside the forest. Nothing prevents this today.

### Solution: Guard in `plan_forest`

After `compute_target_branch` returns the branch name for each repo, check if it matches that repo's `base_branch`. If so, bail with a clear error and hint to choose a different name or use `--branch` / `--repo-branch` to override.

## Implementation status

- [x] Phase 1: Base-branch collision guard (`src/commands/new.rs`)
- [ ] Phase 2: `rm --all` (`src/cli.rs`, `src/commands/rm.rs`, `src/lib.rs`)
- [ ] Phase 3 (optional): Worktree prune retry in `remove_worktree`
