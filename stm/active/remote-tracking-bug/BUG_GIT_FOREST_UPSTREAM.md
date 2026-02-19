# git-forest Bug: Feature branches inherit base branch upstream tracking

## Summary

`git forest new --mode feature` creates feature branches that track `origin/<base-branch>` (e.g., `origin/dev`) as their upstream. Feature branches should either have no upstream set, or track their own remote name (e.g., `origin/dliv/my-feature`).

This causes `git push` to fail with a confusing error about upstream mismatch, requiring the user to manually specify `git push origin HEAD` or fix tracking.

## Version

git-forest 0.2.11

## Reproduction

```sh
# Config: base_branch = "dev", feature_branch_template = "dliv/{name}"
git forest new i18n-cleanup --mode feature

cd ~/worktrees/i18n-cleanup/opencop-web
git branch -vv
# Output:
# * dliv/i18n-cleanup  f29aa99d [origin/dev: ahead 3] ...
#                                ^^^^^^^^^^
#                                should not track origin/dev

git push
# fatal: The upstream branch of your current branch does not match
# the name of your current branch.
```

## Root cause

When git-forest creates the feature branch (presumably via `git worktree add -b dliv/i18n-cleanup origin/dev`), git inherits the tracking configuration from the start point. The branch config ends up as:

```ini
[branch "dliv/i18n-cleanup"]
    remote = origin
    merge = refs/heads/dev    # ← inherited from base branch, should not be set
```

## Expected behavior

Feature branches should be created with **no upstream tracking**. The user sets upstream on first push with `git push -u origin HEAD`, which correctly sets tracking to `origin/dliv/i18n-cleanup`.

Alternatively, git-forest could run `git branch --unset-upstream` after creating each feature branch worktree.

## Workaround

After creating a forest, unset upstream manually:

```sh
cd ~/worktrees/<forest>/<repo>
git branch --unset-upstream
git push -u origin HEAD
```

Or across all repos:

```sh
git forest exec <name> -- git branch --unset-upstream
```
