# Agent UX Fixes — Dry-run on Init + Permission Error Messages

**Goal:** Fix two issues surfaced when Claude Code used git-forest with fresh context.

**Status:** ✅ Complete

---

## Issue 1: `--dry-run` on `init`

### What happened

Claude Code tried `git forest init --dry-run` — that flag doesn't exist on `init`. Clap rejected it.

### Investigation

**Which subcommands have `--dry-run`?** (from `src/cli.rs`)
- `new` — yes (line 72)
- `rm` — yes (line 83)
- `init` — no

**Does `docs/agent-instructions.md` mislead?** 

Line 81: `"Dry-run before mutating: Always run --dry-run --json before new or rm to preview."`

This correctly says `new` or `rm` — it does NOT mention `init`. However, `init` is a mutating command (writes config file), so an agent might reasonably infer `--dry-run` should apply. The docs don't explicitly list `init` as a mutating command without dry-run support.

### Plan

Two separate improvements:

#### 1a. Docs fix (do now)

In `docs/agent-instructions.md`, clarify which commands are mutating and which support `--dry-run`. Change the Agent Best Practices bullet to:

```
- **Dry-run before mutating:** `new` and `rm` support `--dry-run --json` to preview changes. `init` does not support `--dry-run` — it writes/updates a config file (use `--show-path` to see where).
```

#### 1b. Consider `--dry-run` for `init` (future, not now)

`init` writes a config file via `write_config_atomic()`. A `--dry-run` could print the TOML that would be written without writing it. Low priority — `init` is idempotent with `--force`, and `--show-path` lets agents verify the location. Note for future consideration, don't implement now.

---

## Issue 2: `~/.config` permissions error

### What happened

When `~/.config` is owned by root (macOS edge case, e.g. fish installed by root), `git forest init` fails with raw `"Permission denied (os error 13)"` from `create_dir_all`.

### Investigation

**Directory creation callsites that touch XDG paths:**

1. **`src/config.rs:285-288` — `write_config_atomic()`** — creates `~/.config/git-forest/` parent dir. Called by `init` command (`src/commands/init.rs:161`). Uses `create_dir_all` with `.with_context()` — produces `"failed to create config directory ~/.config/git-forest"` but no hint about *why* or how to fix it.

2. **`src/version_check.rs:40-43` — `write_state()`** — creates `~/.local/state/git-forest/` parent dir. Uses `.ok()?` so permission errors are silently swallowed (returns `None`). This is fine — version check is best-effort.

Only `write_config_atomic` is the problem. The `with_context` message says what failed but not why or how to fix.

### Plan

In `write_config_atomic()`, detect `PermissionDenied` on the `create_dir_all` call and add a hint. The fix is in `src/config.rs` around line 287.

**Approach:** Match on `io::ErrorKind::PermissionDenied` from the `create_dir_all` result. When detected, walk up to find which ancestor directory is the permission problem, check its ownership, and produce a hint like:

```
failed to create config directory /Users/foo/.config/git-forest
  hint: /Users/foo/.config is owned by root. Run: sudo chown $(whoami) /Users/foo/.config
```

For non-`PermissionDenied` errors, keep the existing generic context message.

**Implementation detail:** Only need to handle the config dir case (`write_config_atomic`). The state dir case (`write_state` in `version_check.rs`) already swallows errors silently since version checking is best-effort — no change needed there.

**Note on root cause:** The previous thread inferred fish shell created `~/.config` as root, but this is speculation — any `sudo`'d process could have done it. The hint message should be tool-agnostic ("owned by root"), not mention fish.

**Scope:**
- Modify `write_config_atomic()` in `src/config.rs`
- Extract diagnosis into two testable pieces:
  1. **Ancestor-walking function** — given a path, find the first ancestor that blocks creation and inspect its ownership (uses `MetadataExt::uid()` on Unix)
  2. **Hint formatting function** — pure function: takes diagnosis result → produces hint string

**Testing strategy** (no `#[ignore]` needed):
1. **`#[cfg(unix)]` chmod test** — create a tempdir, set a parent to `0o555` (read-only), attempt `create_dir_all` inside it, verify the diagnosis function identifies the correct blocking ancestor. Guard: skip gracefully if running as root (`create_dir_all` succeeds unexpectedly → root bypasses permission checks).
2. **Pure formatting test** — construct diagnosis struct with fake data (uid=0, blocking path), assert hint contains the path and `sudo chown $(whoami)` suggestion. No filesystem needed.

---

## Checklist

1. [x] Update `docs/agent-instructions.md` — clarify `--dry-run` availability
2. [x] Update `src/config.rs` `write_config_atomic()` — better permission error
3. [x] Add `libc` as `cfg(unix)` dependency (already transitive, now direct)
4. [x] Add 3 tests: unwritable parent (chmod), root-owned ancestor (/), edge case (empty path)
5. [x] `just check && just test` — all 212 tests pass
6. [ ] Commit (ask user)
