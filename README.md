# git-forest

Multi-repo worktree orchestrator. If your repos _should_ be a monorepo but aren't, this tool makes worktrees across all of them feel like one.

The code and documentation in this repo were at least 99% LLM generated.

Installs as `git-forest`, invoked as `git forest <command>`.

## Install

```sh
brew tap dliv/tools
brew install git-forest
```

Or download a binary from [GitHub Releases](https://github.com/dliv/workforest/releases).

## AI Agent Integration

git-forest is designed agent-first ([ADR 0001](docs/decisions/0001-agent-drivable-first.md)). All commands accept flags (no interactive prompts), support `--json` output, and include actionable error hints.

**For AI agents:** Run `git forest agent-instructions` for usage guidance.

**For your project's agents:** Add the following to your project's `AGENTS.md` or `CLAUDE.md` so agents know git-forest is available:

```markdown
## git-forest

This project uses `git forest` to manage multi-repo worktrees for feature development and PR review.
When the user asks to create a forest, worktree environment, or review a PR across repos,
run `git forest agent-instructions` for full usage guidance.
```

## Quick Start

```sh
# Build
just build

# Configure repos and worktree base
git forest init \
  --feature-branch-template "user/{name}" \
  --repo ~/foo-api \
  --repo ~/foo-web \
  --repo ~/foo-infra \
  --repo ~/dev-docs \
  --base-branch dev \
  --repo-base-branch dev-docs=main \
  --disposable-root-entry .idea \
  --worktree-base ~/worktrees

# Create a forest for feature work
git forest new my-feature --mode feature

# See what you've got
git forest ls
git forest status my-feature

# Run a command across all repos
git forest exec my-feature -- git log --oneline -3

# Clean up when done
git forest rm my-feature
```

## Commands

```
git forest init     Configure repos and defaults
git forest new      Create a forest (worktrees + branches across all repos)
git forest rm       Remove a forest (worktrees, branches, directory)
git forest ls       List all forests
git forest status   Show git status per repo in a forest
git forest exec     Run a command in each repo of a forest
git forest version  Show version (--check to check for updates)
git forest update   Update git-forest to the latest version
```

All commands support `--json` for structured output.

### `init`

```
git forest init --feature-branch-template <tmpl> --repo <path> [--repo <path>...] [options]

Options:
  --template <name>                   Template name to create or update (default: default)
  --worktree-base <path>              Base directory for forests (default: ~/worktrees)
  --base-branch <branch>              Default base branch (default: dev)
  --feature-branch-template <tmpl>    Feature branch naming template (must contain {name})
  --repo-base-branch <repo=branch>    Per-repo base branch override (repeatable)
  --disposable-root-entry <entry>     Root entry ordinary removal may discard (repeatable)
  --force                             Overwrite existing template by the same name
  --show-path                         Print config path and exit
```

### `new`

```
git forest new <name> --mode <feature|review> [options]

Options:
  --template <name>                   Template to use (default: from config)
  --branch <branch>                   Override branch for all repos
  --repo-branch <repo=branch>         Per-repo branch override (repeatable)
  --no-fetch                          Skip fetching remotes
  --dry-run                           Show plan without executing
```

**Feature mode:** All repos get a branch from the feature branch template (e.g., `user/{name}`) off their base branch.

**Review mode:** All repos get `forest/{name}` branch. Use `--repo-branch` to point specific repos at a PR branch.

### `rm`

```
git forest rm [name] [options]

Options:
  --all                          Remove all forests
  --force                        Force removal of dirty worktrees and unmerged branches
  --discard-root-entry <entry>   Discard one exact root entry for this removal (repeatable)
  --dry-run                      Show what would be removed without executing
```

Removal first checks every repo for known safety blockers before changing that forest.
Dirty or locked worktrees, unsafe branch deletion, unpreserved detached commits,
and required observations that fail leave the whole forest intact. Dirtiness
checks include untracked files and submodule changes even when Git display
settings hide them. Submodule checkouts or stored submodule metadata also
block ordinary removal, including metadata retained after deinitialization;
a never-initialized submodule alone does not. Preview and live removal share
that decision. Unmanaged root files are separate cleanup
residue: they can retain the directory without blocking worktree removal.

`--force` retains its dirty-worktree and unmerged-branch overrides; it does not
unlock worktrees or delete branches checked out elsewhere. `rm --all` checks each
forest independently after strict discovery: a blocked forest stays intact while
healthy forests may be removed, with exit 1 for an incomplete batch.

After preflight passes, cleanup remains best effort. Live Git and filesystem
checks can still refuse a later operation and leave partial results; preflight
is not a transaction or rollback guarantee. `reset` retains its separate cleanup
policy and can still partially clean a forest.

Disposable root entries are exact top-level names, not paths or globs. Values configured by `init` are snapshotted into each new forest's `.forest-meta.toml`; changing the template later does not change an existing forest's deletion authority. There are no built-in defaults.

Use `--discard-root-entry` for an older forest or one-off cleanup after inspecting the entry. Ordinary removal still enforces dirty-worktree and branch safety. A symlinked forest root is always refused, including under `--force`; a disposable symlink inside a real forest is unlinked without following its target.

Always preview cleanup first:

```sh
git forest rm my-feature --dry-run --json
git forest rm my-feature --discard-root-entry .idea --dry-run --json
```

Dot-prefixed entries are not inherently disposable. For example, explicitly authorizing `.env` deletes it.

Inaccessible or offline symlinked worktree bases, inaccessible forest entries, corrupt forest metadata, or metadata staged by an interrupted removal block `rm --all` and prevent `reset` from deleting config/state until recovery is complete. JSON removal refuses non-UTF-8 forest-root names before mutation rather than returning an incomplete path after cleanup.

Named `status`, `exec`, and `rm` operations ignore unreadable metadata in unrelated forest directories while still rejecting an unreadable requested forest. `ls` similarly omits metadata-free dot-prefixed directories as worktree-base administrative state, but continues to list or report dot-prefixed directories that contain valid or unreadable forest metadata.

### `ls`, `status`, `exec`

```
git forest ls
git forest status [name]
git forest exec <name> -- <cmd> [args...]
```

`status` and `rm` auto-detect the current forest when run from inside one.

`ls` summarizes registered Git worktree locks and unavailable lock observations.
`status` reports each repo's lock and its reason above its Git status. Human reasons
escape control characters; JSON preserves the reason. In `status --json`, every
repo has `lock_state` with `type` equal to `locked` (and a nullable `reason`),
`unlocked`, or `unavailable`. In `ls --json`, every forest includes
`locked_worktree_count`, `locked_worktrees` (name and nullable reason),
`lock_unavailable_count`, and `lock_unavailable_repos` (repo names).
An unlocked observation requires a matching Git inventory record; a failed
observation or absent record is unavailable. Lock metadata is independent of
branch and directory health, so a missing worktree can still report a lock.

`rm` also includes the observed lock reason in its refusal message, with control
characters escaped. A reason is explanatory text; a lock without a reason still
blocks removal, including with `--force`.

Successful `status --json` entries retain `status.output` and add `status.summary`:

```json
{
  "type": "available",
  "head_commit": "<observed commit ID>",
  "changed_path_count": 2,
  "upstream": { "type": "not_configured" },
  "base": { "type": "available", "ref": "refs/remotes/origin/main", "ahead": 1, "behind": 3 },
  "commits_not_on_known_remotes": { "type": "available", "count": 1 }
}
```

- `changed_path_count` counts Git change entries: staged and unstaged changes
  to the same path count once, a rename counts once, untracked directories are
  expanded to files, and a dirty submodule counts as one parent entry. Ignored
  files do not count. Explicit scan options can reveal entries hidden or grouped
  by settings in the retained text output.
- `upstream` compares the actual branch with its configured upstream. Its type
  can be `available`, `not_configured`, `detached`, or `unavailable`. A configured
  upstream that is missing or cannot be mapped remains unavailable.
- `base` separately compares with the base remote-tracking ref saved when the
  forest was created. It is available or unavailable; legacy metadata without a
  recorded remote does not imply `origin`.
- `commits_not_on_known_remotes` counts commits reachable from the observed HEAD
  but absent from every locally known remote-tracking history. Publishing to a
  backup remote can make it zero even while ahead of the upstream. No known
  remote refs means unavailable.

Unavailable observations have a `reason` and `message`, never invented zero
counts. A failed structured scan makes the summary unavailable while retaining
successful text; a failed individual comparison/count preserves the other
summary fields. Unborn HEAD has a null `head_commit`, a valid changed-path count,
and unavailable history counts. Missing/error repo results remain unchanged.

These are local observations: status does not fetch. Stale refs, fetch coverage
and shallow history affect the counts; being behind alone does not establish
fast-forward readiness. Comparisons use the observed HEAD commit, but the scans,
refs and worktree are not one atomic snapshot. The extra worktree scan and
history walks add synchronous cost, especially for many untracked files or
submodules. Human status output and exit behavior are unchanged. These fields
do not grant removal permission.

## Development

Start with the [developer documentation](docs/README.md) for setup, commands,
design guidance and the code map. [Development](docs/development.md) owns the
local validation workflow; `just check` is the full Rust gate. Agents should
also read [AGENTS.md](AGENTS.md).

## Design

See the [ADR index](docs/decisions/ADR_INDEX.md) for design decisions and
[architecture rules](docs/architecture.md) for their enforcement. Key principles:

- **Agent-drivable first** ([ADR 0001](docs/decisions/0001-agent-drivable-first.md)) — all inputs as flags, `--json` on every command
- **Commands return data, don't print** ([ADR 0002](docs/decisions/0002-functional-core-imperative-shell.md)) — typed result structs, formatted at the edge
- **Plan/execute for mutations** ([ADR 0003](docs/decisions/0003-plan-execute-split.md)) — `new` and `rm` use plan/execute split with `--dry-run`
- **Self-contained forest meta** ([ADR 0004](docs/decisions/0004-forest-meta-self-contained.md)) — `.forest-meta.toml` captures all resolved values at creation; post-creation commands never re-consult config
