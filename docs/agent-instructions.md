# git-forest — Agent Instructions

Multi-repo worktree orchestrator. Creates isolated worktree environments ("forests") across multiple repositories.

For flag-level details on any command, run `git forest <command> --help`.

## When to Use

Use git-forest when you need an isolated worktree environment across multiple repos:
- **PR review:** Reviewing a PR in one repo but needing the other repos at their default branches to run/test the full system. Even if only one repo has changes, the others need clean checkouts as context.
- **Feature work:** Starting a new feature that may touch one or more repos, without disrupting existing checkouts.
- **Cross-repo commands:** Running the same command (test, build, lint) across all repos in a forest.

## Prerequisites

git-forest must be configured before use. Check with:
```sh
git forest init --show-path
```

If not configured, initialize a template. The first template becomes the default:
```sh
git forest init \
  --template myproject \
  --worktree-base ~/worktrees \
  --base-branch main \
  --feature-branch-template "username/{name}" \
  --repo ~/code/repo-a \
  --repo ~/code/repo-b \
  --repo-base-branch repo-b=develop
```

Add more templates with another `init --template other-name`. Use `--force` to overwrite an existing template.

## Core Workflow

### 1. Create a Forest

**Feature mode** — new feature development:
```sh
git forest new my-feature --mode feature
```
Creates worktrees with branches from the configured template (e.g., `username/my-feature`) off each repo's base branch.

**Review mode** — reviewing an existing PR:
```sh
git forest new review-pr-123 --mode review \
  --repo-branch foo-api=feature/the-pr-branch
```
Creates worktrees with `forest/review-pr-123` branches. Use `--repo-branch` to point specific repos at the PR's actual branch. Other repos get clean checkouts at their base branch.

### 2. Work in the Forest

Worktrees are created under the configured worktree base:
```
~/worktrees/my-feature/
  foo-api/      ← worktree on branch username/my-feature
  foo-web/      ← worktree on branch username/my-feature
```

### 3. Inspect and Execute

```sh
git forest status my-feature          # git status per repo
git forest status                     # auto-detect from cwd
git forest exec my-feature -- make test   # run command in each repo
git forest ls                         # list all forests
```

### 4. Clean Up

```sh
git forest rm my-feature              # remove forest
git forest rm                         # auto-detect from cwd
git forest rm my-feature --force      # force-remove dirty worktrees
```

## Agent Best Practices

- **Always use `--json`** for structured, parseable output on any command.
- **Dry-run before mutating:** `new` and `rm` support `--dry-run --json` to preview changes. `init` does not support `--dry-run` — it writes/updates a config file (use `--show-path` to see where).
- **Error messages include hints:** All errors have `hint:` lines with recovery suggestions.
- **Auto-detection:** `status` and `rm` auto-detect the current forest when run from inside a forest worktree. `exec` always requires a name.
- **Exit codes:** 0 = success, 1 = error. `exec` returns 1 if any repo's command fails. `rm` returns 1 if any cleanup step fails.

## Common Patterns

**Start a cross-repo feature:**
```sh
git forest new ticket-123 --mode feature
git forest exec ticket-123 -- git add -A
git forest exec ticket-123 -- git commit -m "feat: implement ticket-123"
git forest exec ticket-123 -- git push -u origin HEAD
```

**Review a multi-repo PR:**
```sh
git forest new review-pr-456 --mode review \
  --repo-branch api=feature/new-endpoint \
  --repo-branch web=feature/new-ui
# work in ~/worktrees/review-pr-456/api/ and .../web/
git forest rm review-pr-456
```

**Multiple templates** for different project groups:
```sh
git forest new my-feature --mode feature --template project-b
```

## Config Location

```sh
git forest init --show-path    # platform-specific config path
```
