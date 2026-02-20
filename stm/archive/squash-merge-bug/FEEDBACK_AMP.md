# Review: `fix: handle squash-merged branches in git forest rm`

Commit: 7586575

**Overall: Good implementation.** The two-tier fallback logic is correct, the plumbing commands are right, and the 4 new tests cover the main scenarios. A few things to address:

## Issues

### 1. Naming/framing mismatch

Fallback 2 proves "fully pushed, no local-only commits", not "squash-merged." The test `rm_squash_merged_branch_succeeds_via_upstream_fallback` actually tests "pushed but unmerged" (nothing is squash-merged in the test). The function name `can_safely_force_delete` is accurate, but comments and test names overstate what's proven. Consider renaming the test and adjusting the doc comment.

### 2. Missing test for fallback 1

There's no test where the branch is *actually merged* into `base_branch` but `-d` fails because HEAD isn't on base. This is the exact case fallback 1 exists for — worth covering.

### 3. Error message could be more specific

Current message:
```
branch "dliv/foo" is not fully merged
  hint: if it was squash-merged, use `git forest rm --force`
```
The hint assumes squash-merge, but it could also be "never merged at all." Something like:
```
hint: if the branch was merged (e.g. squash-merge) and the remote branch was deleted, use --force
```

### 4. Original `-d` error is discarded

The `Err(_)` at the first `git branch -d` call drops the original git error message. If both fallbacks fail, the user only sees the generic "not fully merged" message, not git's original stderr (which sometimes has useful detail). Consider capturing and including it.

## Looks Good

- `can_safely_force_delete` logic is correct — fails closed on any error
- Using `for-each-ref --format=%(upstream:short)` instead of hardcoding `origin` — follows ADR 0012
- Adding `base_branch` to `RepoRmPlan` from meta is the right data threading
- Force path (`-D` directly) is cleanly separated with early return
- All 4 tests use real git repos per ADR 0007
- All 24 rm tests pass
