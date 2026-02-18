# git-forest: Improve Agent Instructions for Base Branch Selection

## Problem

When an agent uses `git forest agent-instructions` to learn how to run `init`, it pattern-matches off the example, which hardcodes `--base-branch main`. The agent copies `main` without asking the user, even when the project uses `dev` or `develop`.

This happened in practice: Claude Code created a forest with `--base-branch main` for a project that uses `dev`, and the user had to re-run with the correct branch.

## Current Agent Instructions (relevant excerpt)

```bash
git forest init \
  --template myproject \
  --worktree-base ~/worktrees \
  --base-branch main \                # agent copies this literally
  --feature-branch-template "username/{name}" \
  --repo ~/code/repo-a \
  --repo ~/code/repo-b \
  --repo-base-branch repo-b=develop
```

## Suggested Changes

### 1. Add guidance in the agent instructions text

Before or after the `init` example, add something like:

> **Base branch:** Don't assume `main`. Ask the user which base branch to use — common defaults are `main`, `dev`, and `develop`. Different repos in the same template may use different base branches (use `--repo-base-branch` for per-repo overrides).

### 2. Consider auto-detecting the default branch

If `git forest init` could infer the default branch from each repo's remote (e.g. `git symbolic-ref refs/remotes/origin/HEAD`), the `--base-branch` flag could become optional with a sensible default. This would make agents (and humans) less likely to get it wrong.

If auto-detection isn't reliable enough to be the default, it could still be surfaced as a hint:

```
Detected default branches:
  repo-a: dev (from origin/HEAD)
  repo-b: develop (from origin/HEAD)
Use --base-branch to set the template default, or --repo-base-branch for per-repo overrides.
```
