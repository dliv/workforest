# git-forest

Multi-repo worktree orchestrator. Manages collections of git worktrees ("forests") across multiple repositories for parallel development and PR review workflows.

Installs as `git-forest`, invoked as `git forest <command>`.

## Status

Phase 1 complete — read-only commands work. `init`, `new`, and `rm` are stubbed.

## Setup

Requires [just](https://just.systems/man/en/).

```
just setup    # configures git hooks
```

## Build & Test

```
just build
just test
just check    # fmt + clippy
```

## Usage

```
git-forest --help
git-forest ls                    # list all forests
git-forest status [name]         # git status per repo in a forest
git-forest exec <name> -- <cmd>  # run command in each repo
git-forest init                  # not yet implemented
git-forest new <name>            # not yet implemented
git-forest rm [name]             # not yet implemented
```
