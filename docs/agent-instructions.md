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

**Important — detect the base branch first.** Don't assume `main`. Before running `init`, determine each repo's default branch:
```sh
# Check default branch from remote
git -C ~/code/repo-a symbolic-ref refs/remotes/origin/HEAD
# Or ask the user which branch to use
```

```sh
git forest init \
  --template myproject \
  --worktree-base ~/worktrees \
  --base-branch <detected-default-branch> \
  --feature-branch-template "<username>/{name}" \
  --repo ~/code/repo-a \
  --repo ~/code/repo-b \
  --repo-base-branch repo-b=<branch-if-different> \
  --disposable-root-entry .idea
```

The per-repo branch override and disposable root entries are optional; both options are repeatable. Add more templates with another `init --template other-name`. Use `--force` to overwrite an existing template.

Disposable root entries are exact top-level names, not nested paths or globs. They are snapshotted into each new forest's metadata, and there are no built-in defaults.

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

With `--json`, `ls` returns both `forests` and `findings`. Always inspect
`findings`: `missing-metadata` identifies a directory under a configured
worktree base with no `.forest-meta.toml`, while `unreadable-metadata`
identifies metadata that could not be read, parsed, or validated. These are
observations only; they do not make the directory a managed forest or grant
cleanup authority. Finding paths are display-safe strings; a non-UTF-8 name
may contain replacement characters but cannot suppress the rest of the JSON
inventory. A dot-prefixed directory without metadata is treated as tool or
administrative state and omitted; valid or unreadable forest metadata inside a
dot-prefixed directory remains observable.

### 4. Clean Up

```sh
git forest rm my-feature --dry-run --json  # inspect every planned deletion first
git forest rm my-feature              # remove forest
git forest rm                         # auto-detect from cwd
git forest rm my-feature --force      # force-remove dirty worktrees
git forest rm my-feature --discard-root-entry .idea --dry-run --json
git forest reset --confirm            # wipe all config, state, and forests
git forest reset --config-only --confirm  # wipe config/state only, keep worktrees
```

## Agent Best Practices

- **Always use `--json`** for structured, parseable output on any command.
- **Dry-run before mutating:** `new`, `rm`, and `reset` support `--dry-run --json` to preview changes. `init` does not support `--dry-run` — it writes/updates a config file (use `--show-path` to see where).
- **Treat disposable entries as deletion authority:** Configured entries are recursively deleted during ordinary `rm`. For older forests, inspect the entry and use repeatable `--discard-root-entry <entry>` only when its complete contents may be discarded.
- **Dotfiles are not automatically safe:** Explicitly passing `.env` to `--discard-root-entry` deletes it. Never infer disposable entries merely because their names begin with a dot.
- **Symlink boundary:** `rm` refuses a symlinked forest root even with `--force`. Inside a real forest, deleting an authorized symlink removes the link rather than following its target.
- **Recovery blockers are intentional:** Inaccessible or offline symlinked worktree bases, inaccessible forest entries, corrupt forest metadata, or a staged metadata file left beside the worktree base block `rm --all` and prevent `reset` from deleting config/state. Preserve and repair the reported state before retrying.
- **Named operations are scoped:** `status <name>`, `exec <name>`, and `rm <name>` ignore unreadable metadata in unrelated forest directories. They still reject unreadable metadata for the requested forest and command-level base inspection failures.
- **Inspect inventory findings:** `git forest ls --json` continues past missing or unreadable metadata and exits 0 after producing the inventory, even when no readable forests exist. A command-level failure to enumerate a configured worktree base still exits 1.
- **JSON requires representable paths:** `rm --json` refuses non-UTF-8 forest-root names before mutation. Inspect and rename the reported entry; do not silently retry the destructive command without JSON.
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
# diff against origin/<base>, not local <base> — local may be stale
# git diff origin/main...feature/new-endpoint
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
