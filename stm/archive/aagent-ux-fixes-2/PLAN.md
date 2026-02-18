# Agent UX Fixes — Round 2

Two changes: fix the base branch problem in agent instructions, and add a `reset` command.

## 1. Agent Instructions: Base Branch Guidance

**Problem:** The `init` example hardcodes `--base-branch main`. Agents copy it literally without asking the user. This already caused a real mis-init.

**Fix (docs only, no code change):**

In both `docs/agent-instructions.md` and `.agents/skills/using-git-forest/SKILL.md`:

- Remove `--base-branch main` from the example
- Add guidance telling the agent to verify base branches before running init. The agent has the tools to do this — it can run `git symbolic-ref refs/remotes/origin/HEAD` or `git branch -r` in each repo to detect the default branch, or just ask the user. The instructions should tell it to do one of these rather than guess.
- Use a placeholder in the example that makes it obvious the value needs to be determined:

```sh
git forest init \
  --template myproject \
  --worktree-base ~/worktrees \
  --base-branch <detected or ask user> \
  --feature-branch-template "<user's preferred prefix>/{name}" \
  --repo ~/code/repo-a \
  --repo ~/code/repo-b \
  --repo-base-branch repo-b=<branch if different from base-branch>  # optional
```

The key insight: we don't need auto-detection in the CLI because the agents already have shell access. Just tell them to check.

## 2. `git forest reset` Command

**Problem:** No way to cleanly wipe all git-forest state. Useful when:
- Config gets into a bad state (e.g., old format, wrong paths)
- Worktree base has orphaned forests
- State file has stale data
- Testing / starting fresh

**What it removes:**

| Path | Contents |
|------|----------|
| `~/.config/git-forest/config.toml` | All templates |
| `~/.config/git-forest/state.toml` | Version check cache |
| Worktree base dirs from config | All forest directories and worktrees |

**Behavior:**

- `git forest reset` — interactive: print what will be deleted, require `--confirm` or `--force`
- `git forest reset --confirm` — delete everything, print summary
- `git forest reset --config-only` — delete config + state files but leave worktree directories intact
- Always prints what was deleted
- Supports `--json` (like all commands)
- Supports `--dry-run` to preview

**Implementation:**

1. Add `Reset` variant to `Command` enum in `src/cli.rs`:
   ```rust
   Reset {
       #[arg(long)]
       confirm: bool,
       #[arg(long)]
       config_only: bool,
       #[arg(long)]
       dry_run: bool,
   }
   ```

2. New module `src/commands/reset.rs`:
   - Read config to discover worktree base paths
   - `rm` all forests (reuse existing `rm` logic or just remove the worktree base dirs)
   - Delete `config.toml` and `state.toml`
   - If config can't be parsed (the "bad state" case), still delete the config files and warn that worktree dirs need manual cleanup

3. Wire up in `src/main.rs` match arm

**Edge cases:**
- Config doesn't exist → just delete state.toml if present, print "nothing to reset"
- Config exists but can't be parsed → delete config files, warn about worktree dirs
- Worktree base doesn't exist → skip, no error
- Forests have dirty worktrees → require `--force` (same as `rm`)

## Order of Work

1. Agent instructions fix (docs only, quick)
2. `reset` command (code change, needs tests)
3. `just check && just test`
4. Commit, bump, tag, push
