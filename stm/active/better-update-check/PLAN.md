# Plan: Non-blocking version check

## Overview

Today, `check_for_update()` does a synchronous HTTP fetch (500ms timeout) that blocks the user. We'll replace it with a cache-read + detached-subprocess-spawn model so no command ever waits on the network for the version check.

## Step 1: Split state file reads and writes

**File:** `src/version_check.rs`

Currently `write_state` always writes both `last_checked` and `latest_version` together. We need:

- `write_last_checked(now)` — writes/updates only `last_checked` in the state file (parent process calls this before spawning)
- `write_latest_version(version)` — writes/updates only `latest_version` (subprocess calls this after fetch)

This avoids timestamp races between parent and child. The `StateFile` struct may need adjustment — possibly separate the two fields so they can be independently updated (read-modify-write the TOML, preserving the other field).

## Step 2: New public entry point `check_cache_and_notify`

**File:** `src/version_check.rs`

Replace `check_for_update(debug)` with a new function (non-blocking):

```
pub fn check_cache_and_notify(debug: bool)
```

Logic:
1. If not `is_enabled()`, return
2. Read state file
3. **No state file / no `latest_version`** → first run. Do a synchronous fetch (acceptable), print the first-run notice, write full state. Return.
4. **Cached `latest_version` is newer** → print update notice (from cache, no network)
5. **Cache is stale** (>24h) → write `last_checked = now` immediately, then spawn detached subprocess via `--internal-version-check`
6. **Cache is fresh** → do nothing

This replaces the current `check_for_update` which returns `Option<UpdateNotice>` — the new function handles printing internally (simpler call site in main).

## Step 3: Add subprocess entry point

**File:** `src/version_check.rs`

```
pub fn run_background_version_check()
```

- Called when the binary is invoked with `--internal-version-check`
- Calls `fetch_latest_version` with a 5s timeout (instead of 500ms — not blocking anyone)
- On success, writes `latest_version` to state file (preserving existing `last_checked`)
- On failure, exits silently (the parent already wrote `last_checked`, so retry won't happen for 24h)
- Suppresses all output

## Step 4: Intercept `--internal-version-check` in main

**File:** `src/main.rs`

Before `Cli::parse()`, check `std::env::args()` for `--internal-version-check`:

```rust
fn main() {
    if std::env::args().any(|a| a == "--internal-version-check") {
        version_check::run_background_version_check();
        return;
    }
    // existing Cli::parse() and run() ...
}
```

This keeps it out of clap's subcommand model — it's an internal implementation detail.

## Step 5: Spawn the detached subprocess

In `check_cache_and_notify`, when the cache is stale:

```rust
if let Ok(exe) = std::env::current_exe() {
    let _ = std::process::Command::new(exe)
        .arg("--internal-version-check")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Drop the Child handle — don't wait
}
```

## Step 6: Update `force_check` message

**File:** `src/main.rs` (the `Command::Version { check }` arm)

Remove the confusing `"You are up to date (or the update server is unreachable)."` message. Replace with two distinct cases:
- Network success + up to date → `"You are up to date."`
- Network failure → `"Could not reach the update server."`

This requires `force_check` to return a richer type (e.g., `Result<Option<UpdateNotice>, ...>` or an enum) so the caller can distinguish "up to date" from "fetch failed".

## Step 7: Wire up main.rs call site

Replace:
```rust
if let Some(notice) = version_check::check_for_update(debug) {
    eprintln!("Update available: ...", notice.latest, notice.current);
}
```

With:
```rust
version_check::check_cache_and_notify(debug);
```

## Files changed

| File | Changes |
|---|---|
| `src/version_check.rs` | Split read/write, add `check_cache_and_notify`, add `run_background_version_check`, enrich `force_check` return type, bump subprocess timeout to 5s |
| `src/main.rs` | Intercept `--internal-version-check`, replace `check_for_update` call |

## Edge cases (from prompt, confirmed)

- First run (no state file) → synchronous check + notice *(only acceptable blocking case)*
- `latest_version` missing from state → treat as first run
- Subprocess can't write → silent failure, retries in 24h
- Concurrent commands → first writes `last_checked`, others see fresh cache
- `current_exe()` fails → skip background check silently

## Not changing

- No new dependencies
- `is_enabled()` logic stays the same
- The set of commands that trigger the check stays the same
- `git forest update` is unaffected
