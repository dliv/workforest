---
name: using-git-forest
description: "Manages multi-repo worktrees using the git-forest CLI (invoked as `git forest`). Use when the user mentions workforest, forest, worktrees across repos, or needs isolated worktree environments for feature work or PR review across multiple repositories."
---

# Using git-forest

Multi-repo worktree orchestrator. Creates isolated worktree environments ("forests") across multiple repositories.

For comprehensive usage guidance, run `git forest agent-instructions`.
For flag details on any command, run `git forest <command> --help`.

## When to Use

Use git-forest when the user needs an isolated worktree environment across multiple repos:
- **PR review:** Reviewing a PR in one repo but needing the other repos at their default branches to run/test the full system. Even if only one repo has changes, the others need clean checkouts as context.
- **Feature work:** Starting a new feature that may touch one or more repos, without disrupting existing checkouts.
- **Cross-repo commands:** Running the same command (test, build, lint) across all repos in a forest.

## Quick Reference

```sh
git forest init --show-path                     # check if configured
git forest new <name> --mode feature            # create forest for feature work
git forest new <name> --mode review             # create forest for PR review
git forest new <name> --mode feature --dry-run --json  # preview before creating
git forest status [name]                        # git status per repo
git forest exec <name> -- <cmd> [args...]       # run command across repos
git forest ls                                   # list all forests
git forest rm [name]                            # clean up a forest
```

## Key Concepts

- **Feature mode:** Creates branches from the configured template (e.g., `username/{name}`) off each repo's base branch.
- **Review mode:** Creates `forest/{name}` branches. Use `--repo-branch repo=branch` to point specific repos at a PR's actual branch.
- **Auto-detection:** `status` and `rm` auto-detect the current forest when run from inside one.
- **All commands support `--json`** for structured output.
- **Always `--dry-run --json` before `new` or `rm`** to preview changes.
- **Errors include `hint:` lines** with recovery suggestions.
