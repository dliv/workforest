# git-forest Bug: rm uses stale local base branch for safe deletion

## Summary

`git forest rm` can fail to delete forest-created branches when the branch is fully
contained in the remote-tracking base branch, but the local base branch is stale.

This is distinct from the older squash-merge bug. In this case the generated
forest branch is an ancestor of `origin/main`, so the work is already contained
in the remote base. The false "not fully merged" result comes from checking
against local `main`.

Observed with `git-forest 0.4.0`.

## Real incident

While cleaning old OpenCOP review forests, `git forest rm
tree-mixed-historical-subjects-mr214-review` partially succeeded:

- removed the worktrees
- deleted the `dlivoc` generated branch
- left `.forest-meta.toml` behind for discoverability
- failed to delete generated branches in `opencop-web` and `opencop-infra`

Error shape:

```text
opencop-web: branch "forest/tree-mixed-historical-subjects-mr214-review" is not fully merged
opencop-infra: branch "forest/tree-mixed-historical-subjects-mr214-review" is not fully merged
```

The forest metadata for generated review branches records only:

```toml
branch = "forest/tree-mixed-historical-subjects-mr214-review"
base_branch = "main"
branch_created = true
```

Manual checks showed the branches were not ahead of `origin/main`:

```text
opencop-web:
  main..forest/tree-mixed-historical-subjects-mr214-review        = 30
  origin/main..forest/tree-mixed-historical-subjects-mr214-review = 0

opencop-infra:
  main..forest/tree-mixed-historical-subjects-mr214-review        = 4
  origin/main..forest/tree-mixed-historical-subjects-mr214-review = 0
```

So the local base branch was stale; the remote-tracking base branch already
contained the generated forest branch.

## Current code path

`src/commands/rm.rs`:

1. `delete_branch()` tries `git branch -d <branch>`.
2. If that fails, `can_safely_force_delete(source, branch, base_branch)` runs.
3. Fallback 1 checks:

```text
git merge-base --is-ancestor <branch> <base_branch>
```

4. Fallback 2 checks whether the branch has an upstream and no unpushed commits.

This misses review-mode generated branches that:

- do not have their own upstream, and
- are fully contained in `origin/<base_branch>`, but
- are not contained in stale local `<base_branch>`.

## Expected behavior

When a forest-created branch is an ancestor of the base branch's remote-tracking
ref, `git forest rm` should treat it as safe to delete without requiring
`--force`.

For the incident above, `rm` should have removed:

- `forest/tree-mixed-historical-subjects-mr214-review` in `opencop-web`
- `forest/tree-mixed-historical-subjects-mr214-review` in `opencop-infra`

because both were contained in `origin/main`.

## Proposed fix

Extend `can_safely_force_delete` with a remote-tracking base check between the
local base ancestry check and the branch-upstream check.

Preferred order:

1. `git merge-base --is-ancestor <branch> <base_branch>`
2. Resolve the base branch's upstream, for example:

```text
git for-each-ref --format=%(upstream:short) refs/heads/<base_branch>
```

Then, if non-empty:

```text
git merge-base --is-ancestor <branch> <base_upstream>
```

3. If the local base branch has no upstream, fall back to a configured/default
   remote-tracking ref such as `origin/<base_branch>` when it exists.
4. Existing branch-upstream/no-unpushed-commits check.

This keeps the safety property: only force-delete when Git can prove the branch
tip is reachable from a known base ref, or when the existing pushed-branch
fallback proves there are no local-only commits.

## Test idea

Create a source repo where:

1. local `main` is intentionally behind `origin/main`
2. `forest/review-x` was created from an older `origin/main`
3. `origin/main` advances to include `forest/review-x`
4. local `main` remains stale
5. `git branch -d forest/review-x` fails
6. `git merge-base --is-ancestor forest/review-x main` fails
7. `git merge-base --is-ancestor forest/review-x origin/main` succeeds

Expected: `git forest rm review-x` deletes the generated branch and removes the
forest directory without requiring `--force`.

Also keep a negative test where `forest/review-x` has a commit that is not in
local `main`, not in the base upstream, and not pushed to its own upstream.
Expected: non-force `rm` still fails and preserves the branch.

## Relationship to prior bugs

- `stm/archive/squash-merge-bug/BUG_GIT_FOREST_SQUASH_MERGE.md` covers
  squash-merged feature branches where original commits are not reachable from
  the target branch.
- This report covers regular reachability through the base branch's
  remote-tracking ref when local base is stale.
- Both surface as "not fully merged", but the safe-deletion proofs are different.
