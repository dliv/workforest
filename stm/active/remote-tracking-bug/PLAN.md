# Plan: Fix feature branches inheriting base branch upstream tracking

## Overview

`git forest new --mode feature` creates branches that unintentionally track `origin/<base-branch>` (e.g., `origin/dev`) because `git worktree add -b <branch> origin/<base>` auto-sets upstream tracking when the start point is a remote ref. Fix: add `--no-track`.

## Change

**File:** `src/commands/new.rs`, `execute_plan()`, `CheckoutKind::NewBranch` arm (line 313-316)

Current:
```rust
&["worktree", "add", &dest_str, "-b", branch_str, &start]
```

Fixed:
```rust
&["worktree", "add", &dest_str, "-b", branch_str, "--no-track", &start]
```

## What about the other cases?

- **`TrackRemote`** (line 306-308): Branch exists on remote with the same name. Tracking `origin/<branch>` is correct here — the user is checking out an existing remote branch. **No change.**
- **`ExistingLocal`** (line 299-302): Checks out an existing local branch. Doesn't create a branch or change upstream. **No change.**

## Test

Add an integration test in `tests/cli_test.rs` that:

1. Creates a forest with `--mode feature`
2. Verifies the feature branch has **no upstream** set (`git config branch.<name>.merge` returns nothing)

Can use the existing `setup_new_env()` helper which already creates repos with remotes.
