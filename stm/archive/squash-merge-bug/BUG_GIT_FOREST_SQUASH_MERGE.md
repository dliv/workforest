# git-forest issue: rm should fall back to branch -D when branch is pushed but squash-merged

## Problem

`git forest rm` uses `git branch -d` to clean up local branches. When a branch was squash-merged (common workflow — GitLab/GitHub default), `-d` fails with "not fully merged" because the original commits aren't reachable from the target branch. The squash commit has a different SHA.

The worktree is removed successfully, but the local branch is left behind:

```
$ git forest rm i18n-cleanup
opencop-web: git branch -d failed
  error: the branch 'dliv/i18n-cleanup' is not fully merged
```

This will happen every time for teams that use squash merges (which is most teams).

## Proposed fix

When `git branch -d` fails, check if the branch is safe to force-delete:

1. Branch has a remote tracking ref (it was pushed)
2. Branch is not ahead of the remote (no unpushed local commits)

If both are true, fall back to `git branch -D`. The work is safely on the remote — the `-d` failure is a false positive caused by squash merge rewriting history.

If the branch **is** ahead of its remote, keep the current error. That's a real warning — unpushed work would be lost.

## Context

- The `rm` dry-run already confirms "no dirty worktrees" before proceeding
- A clean worktree + pushed branch = zero data loss risk from force-deleting the local ref
- Squash merge is the default on GitHub and common on GitLab, so this will affect most users

## Reproduction

```bash
# setup: create forest, push branch, squash-merge via MR/PR, then:
git forest rm <forest-name>
# branch deletion fails for the squash-merged repo
```
