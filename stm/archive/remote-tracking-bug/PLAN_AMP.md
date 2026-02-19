# Implementation Notes (Amp)

**Status:** ✅ Complete

## What changed vs. the original plan

### 1. Argument ordering

The plan had `--no-track` inserted in-place:
```rust
&["worktree", "add", &dest_str, "-b", branch_str, "--no-track", &start]
```

We used canonical ordering instead (options before positional args) for compatibility across git versions:
```rust
&["worktree", "add", "-b", branch_str, "--no-track", &dest_str, &start]
```

Some git versions may not correctly parse options that appear after positional arguments like `<path>`. The git docs show: `git worktree add [-b <new-branch>] [--no-track] <path> [<commit-ish>]`.

### 2. Stronger test assertions

The plan proposed checking `git config branch.<name>.merge` returns nothing. We instead assert `git rev-parse --abbrev-ref @{u}` fails — this is a behavioral check that covers both `branch.<name>.merge` and `branch.<name>.remote` being unset.

### 3. Added regression test for TrackRemote

The plan only tested the fix (NewBranch has no upstream). We added `review_branch_tracks_remote` to verify `TrackRemote` still correctly sets upstream to `origin/<branch>`. This guards against accidentally breaking the other code path.

### 4. Tests in unit tests, not integration tests

The plan said to add tests in `tests/cli_test.rs`. We put them in `src/commands/new.rs` alongside the existing `cmd_new_*` tests, which already have the `TestEnv` infrastructure for creating repos with remotes.

### 5. Added code comment

Added a comment on the `NewBranch` arm explaining why we use canonical argument ordering.
