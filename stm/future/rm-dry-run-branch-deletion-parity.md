# git-forest Bug: rm dry-run over-promises branch deletion success

## Summary

`git forest rm <name> --dry-run --json` can report `branch_deleted: success`
for a branch that actual non-force `git forest rm <name>` will fail to delete.

Observed with `git-forest 0.4.2`.

This is a dry-run parity bug. It is not the same as the stale-local-base bug in
`stm/future/rm-stale-local-base-branch.md`; that bug was fixed by checking the
base branch's remote-tracking ref. This report covers the remaining behavior
where dry-run marks branch deletion as successful without proving that the
actual branch deletion path will succeed.

## Real incident

While cleaning the archived OpenCOP forest `sdy9g-flaky-test-fix`, dry-run said
all branches would be deleted:

```json
{
  "forest_name": "sdy9g-flaky-test-fix",
  "dry_run": true,
  "repos": [
    {
      "name": "opencop-java",
      "worktree_removed": { "status": "success" },
      "branch_deleted": { "status": "success" }
    }
  ],
  "forest_dir_removed": true,
  "errors": []
}
```

Actual non-force removal then partially succeeded and failed deleting the Java
branch:

```text
Removing forest "sdy9g-flaky-test-fix"
  opencop-java: removing... worktree removed, branch FAILED
  opencop-web: removing... worktree removed, branch deleted
  opencop-infra: removing... worktree removed, branch deleted
  dlivoc: removing... worktree removed, branch deleted
Forest directory not removed (not empty).

Errors:
  opencop-java: branch "dliv/sdy9g-flaky-test-fix" is not fully merged
```

The result left:

- all worktrees removed
- Java branch `dliv/sdy9g-flaky-test-fix` still present
- forest directory present with only `.forest-meta.toml`

## Why actual rm failed

GitLab MR !202 was merged and the remote source branch was deleted:

```text
MR !202 state: merged
source_branch: dliv/sdy9g-flaky-test-fix
merged_at: 2026-06-12T18:18:50.601Z
sha: 1e4ebbe9a943a02f8d0b970cba496002f3e2542c
merge_commit_sha: caef989a94f646bc34a81cbc3fb07197fa8dd11c
squash_commit_sha: a61afe38e2cfbbffdee121109698a58d18cfc1a4
```

The local branch was not an ancestor of `origin/main` and the remote upstream was
gone:

```text
git branch -vv --list dliv/sdy9g-flaky-test-fix
  dliv/sdy9g-flaky-test-fix 1e4ebbe9 [origin/dliv/sdy9g-flaky-test-fix: gone]
```

That means actual non-force `rm` could not prove safe deletion through:

- `git branch -d`
- base branch ancestry
- base remote-tracking ancestry
- branch upstream "no local-only commits" check

It correctly failed closed and suggested `--force`.

## Current code path

`src/commands/rm.rs` has two different levels of fidelity:

- `delete_branch()` performs the real checks: `git branch -d`, already-deleted
  guard, `can_safely_force_delete(...)`, then fail closed.
- `plan_to_dry_run_result()` does not evaluate branch deletion feasibility. For
  any `branch_created` repo without a dirty-worktree block, it returns
  `RmOutcome::Success`.

Relevant dry-run behavior:

```rust
let branch_deleted = if !rp.branch_created {
    RmOutcome::Skipped {
        reason: "branch not created by forest".to_string(),
    }
} else if !rp.worktree_exists {
    RmOutcome::Success
} else {
    RmOutcome::Success
};
```

This makes dry-run useful for directory and dirty-worktree checks, but not for
branch deletion safety.

## Expected behavior

Dry-run should not report branch deletion success when actual non-force removal
would fail.

Reasonable output choices:

1. Prefer high-fidelity dry-run: run read-only checks equivalent to the real
   branch-deletion proof.
   - If `branch -d` would succeed, report success.
   - If branch deletion would be safe only through fallback `-D`, report success
     or a new "safe force-delete fallback" reason.
   - If neither proof succeeds, report failed with the same "not fully merged"
     class of error actual `rm` would produce.
2. If high-fidelity proof is too expensive or too invasive, stop calling it
   `success`; return a distinct skipped/unknown outcome such as "would attempt
   branch delete" so agents do not treat dry-run as approval to mutate.

For agent safety, option 1 is better. Agents currently use `--dry-run --json`
as the gate before destructive cleanup.

## Test idea

Create a forest with a forest-created branch whose worktree is clean, but whose
branch cannot be safely deleted non-force:

1. Create a forest-created branch.
2. Add commits on that branch.
3. Ensure the branch is not an ancestor of local base or remote-tracking base.
4. Ensure the branch has no usable upstream, or has an upstream that is gone.
5. Run `git forest rm <name> --dry-run --json`.

Expected: dry-run reports a branch deletion failure or unknown, and includes an
error/hint. It must not report `branch_deleted: success` with empty `errors`.

Then verify actual `git forest rm <name>` also fails closed without `--force`.

Also add a parity test for the stale-local-base fixed case:

1. Branch is not in local `main`.
2. Branch is in `origin/main`.
3. Dry-run reports success.
4. Actual `rm` succeeds.

## Notes

Dry-run is an agent-facing contract. A false positive is worse than a vague
result because it encourages an agent to begin destructive work and can leave a
forest partially removed when actual execution discovers the branch deletion
problem.
