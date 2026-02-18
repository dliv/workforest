# Non-blocking version check

## Problem

The version check currently runs synchronously after each command. It has a 500ms timeout, but that still blocks the user. If the server is slow or unreachable, the user sees "You are up to date (or the update server is unreachable)" which is confusing — it conflates two different situations.

The check only runs once per day (controlled by a `last_checked` timestamp in the state file), but the timestamp is written *after* a successful network call. If the call keeps failing, it retries on every command invocation.

## Goal

Make the version check fully non-blocking. No command should ever wait on a network call for the version check.

## Design

**Detached subprocess approach:**

1. At the end of a command (where `check_for_update` is called today), the main process:
   - Reads the state file to check staleness
   - If cached `latest_version` exists and is newer than current, prints the update notice (from cache — no network)
   - If the cache is stale (>24h), writes `last_checked = now` to the state file immediately (prevents other commands from also spawning a check), then spawns a detached subprocess to do the actual fetch
   - If the cache is fresh, does nothing

2. The detached subprocess (`std::process::Command::new(current_exe()).arg("--internal-version-check")`):
   - Runs the HTTP fetch with a longer timeout (e.g., 5s instead of 500ms — it's not blocking anyone)
   - On success, writes `latest_version` to the state file (preserving the `last_checked` the parent already wrote)
   - On failure, does nothing — the stale `last_checked` means it won't retry for another 24h
   - Exits silently

3. The `--internal-version-check` flag is intercepted early in `main()` before CLI parsing. It's not a real subcommand — it's an internal implementation detail.

## Key details

- The parent writes `last_checked` *before* spawning the subprocess. This is the "once per day" guarantee — even if the subprocess fails or is killed, we don't retry until tomorrow.
- The subprocess only writes `latest_version`. No timestamp race.
- The first-run notice ("git-forest checks for updates daily...") should still be shown synchronously on the very first run (no state file exists yet). That's the only time a blocking check is acceptable.
- `git forest version --check` (`force_check`) remains synchronous and blocking — the user explicitly asked for it.
- The confusing "up to date (or server unreachable)" message should be removed. If we have a cached version and it's not newer, say nothing. If we have no cached version yet, the first-run path handles it.
- stdout/stderr of the subprocess should be suppressed (already the case with `Command::new(...).spawn()` defaults if we don't pipe them, but verify).
- The subprocess should be fully detached — parent doesn't wait for it. Drop the `Child` handle immediately after spawn.

## Files to change

- `src/version_check.rs` — split into "check cache + maybe spawn" (called by main) and "fetch + write" (called by subprocess)
- `src/main.rs` — intercept `--internal-version-check` before `Cli::parse()`, wire up the new non-blocking check
- No new dependencies needed

## Edge cases

- State file doesn't exist yet (first run) → synchronous check with notice, as today
- State file exists but `latest_version` is missing → treat as first run
- Subprocess can't write state file (permissions) → silent failure, retries next day
- Multiple commands run concurrently → first one writes `last_checked`, others see fresh cache, only one subprocess spawns
- `current_exe()` fails (rare) → skip the background check, no error shown
