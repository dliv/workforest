# Plan: Reset Performance & Release Workflow Improvements

## Overview

Three CLI fixes for perceived slowness in `git forest reset --confirm`, plus server-side improvements to eliminate cold-start latency, plus a `just release` workflow to tie it all together.

### CLI changes
1. **Skip version check for `reset`** — eliminates up to 500ms+ blocking after reset deletes the state file
2. **Unify version check into a single non-blocking path** — missing state is just maximally stale, use the same background subprocess
3. **Incremental output during forest deletion** — user sees progress as each forest is removed

### Server-side changes
4. **Optimize the Cloudflare Worker** — move D1 write out of response path, hardcode version in env instead of KV
5. **`just release` workflow** — single command for version bump, commit, tag, push, worker deploy

---

## Change 1: Skip version check for `reset`

**File:** `src/main.rs`, lines 28–37

**Verified:** The `should_version_check` match at line 28 includes `Command::Reset { .. }` at line 33. This is the only place that gates the version check — the call at line 44–46 uses this bool.

**Change:** Delete the `| Command::Reset { .. }` arm (line 33). Single line removal. The surrounding arms (`Command::Rm { .. }` at line 32 and `| Command::Ls` at line 34) stay.

```rust
// Before (lines 28-37):
let should_version_check = matches!(
    cli.command,
    Command::Init { .. }
        | Command::New { .. }
        | Command::Rm { .. }
        | Command::Reset { .. }    // ← delete this line
        | Command::Ls
        | Command::Status { .. }
        | Command::Exec { .. }
);
```

No other code changes needed — `reset` simply won't trigger the post-command version check.

### Test

Add a CLI integration test in `tests/cli_test.rs`. The test needs to:
1. Set up a fake HOME with a config + state file (so `reset --confirm` has something to delete)
2. Run `git forest reset --confirm`
3. Verify the state file was deleted (by `reset`) and **not** recreated (by version check)

Pattern to follow: `version_check_first_run_shows_privacy_notice` (line 1084) uses `write_version_state()` / `read_version_state()` helpers already defined in that file (lines 915–936). The test should use the same `config_env()` / `CLEARED_XDG_VARS` isolation pattern (lines 37–41).

```rust
#[test]
fn reset_confirm_does_not_trigger_version_check() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // Write a config so reset has something to delete
    let config_dir = fake_home.join(".config").join("git-forest");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), r#"
default_template = "default"
[template.default]
worktree_base = "/tmp/nonexistent"
base_branch = "main"
feature_branch_template = "test/{name}"
[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#).unwrap();

    // Write a version state file
    write_version_state(&fake_home, "2020-01-01T00:00:00Z", Some("0.0.1"));

    cargo_bin_cmd!("git-forest")
        .args(["reset", "--confirm"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .assert()
        .success();

    // State file should be gone (deleted by reset) and NOT recreated by version check
    assert!(read_version_state(&fake_home).is_none(),
        "version check should not run after reset");
}
```

---

## Change 2: Unify version check into a single non-blocking path

**File:** `src/version_check.rs`, function `check_cache_and_notify()` (lines 179–259)

### Current code structure (verified)

The function has two distinct paths:

1. **Synchronous `needs_sync` path** (lines 192–231): Triggered when `state.is_none()` (no state file) OR `state.latest_version.is_none()`. Calls `fetch_latest_version(current, debug, Duration::from_millis(500))` — a **blocking** HTTP call with 500ms timeout. On success, writes state with `latest_version: Some(...)`. On failure, writes state with `latest_version: None` — which means the **next** command hits `needs_sync` again (line 194: `s.latest_version.is_none()` is true).

2. **Non-blocking stale-cache path** (lines 234–258): Reached only when state exists AND `latest_version` is `Some`. Reads from cache, prints notice if newer, spawns `spawn_background_check()` if `is_stale()`.

### The bug

The `needs_sync` path on failure (lines 223–229) writes `latest_version: None`, creating an infinite retry loop: every command blocks for 500ms, fails (cold start > 500ms), writes `None`, next command blocks again. This compounds with Change 1's problem — `reset` deletes the state file entirely, forcing the next command into `needs_sync` too.

### Proposed replacement

Replace the entire function body (lines 179–259) with a single non-blocking path:

```rust
pub fn check_cache_and_notify(debug: bool) {
    if !is_enabled() {
        if debug {
            eprintln!("[debug] version check: disabled in config");
        }
        return;
    }

    let current = env!("CARGO_PKG_VERSION");
    let state = read_state();

    if state.is_none() {
        // First run: show privacy notice, seed state, spawn background check
        if debug {
            eprintln!("[debug] version check: state file not found, first run");
        }
        eprintln!(
            "Note: git-forest checks for updates daily (current version sent to forest.dliv.gg)."
        );
        eprintln!("Disable: set version_check.enabled = false in config.");
        write_state(&VersionCheckState {
            last_checked: Utc::now(),
            latest_version: None,
        });
        spawn_background_check();
        return;
    }

    let cached = state.unwrap();

    // Show update notice from cache if available
    if let Some(ref latest) = cached.latest_version {
        if version_newer(latest, current) {
            eprintln!(
                "Update available: git-forest v{} (current: v{}). Run `git forest update` to upgrade.",
                latest, current
            );
        }
    }

    // Refresh if stale (>= 24h) or if latest_version is still None (previous bg check failed)
    if cached.latest_version.is_none() || is_stale(&cached.last_checked) {
        if debug {
            eprintln!("[debug] version check: cache stale or incomplete, spawning background check");
        }
        update_state(|s| s.last_checked = Utc::now());
        spawn_background_check();
    } else if debug {
        eprintln!(
            "[debug] version check: cache fresh, latest={:?}",
            cached.latest_version
        );
    }
}
```

Key difference from current code: **no call to `fetch_latest_version()` anywhere in this function**. The only caller of `fetch_latest_version()` becomes `run_background_version_check()` (line 145) and `force_check()` (line 264). The 500ms synchronous HTTP timeout is completely eliminated from the normal command path.

### Tests that need updating

**`version_check_missing_latest_version_does_sync_check`** (cli_test.rs line 1112): Currently this test writes state with `latest_version: None` and expects the sync path to run. After this change, the `latest_version: None` case will use the background path instead. The test should verify:
- No privacy notice shown (state file existed)
- State file still exists
- `last_checked` timestamp is updated (background check was spawned)
- No `fetch_latest_version` blocking call (implicitly verified by the timestamp update + no network-dependent assertions)

The test assertions at lines 1130–1136 already check these things correctly — the test should pass as-is because:
- Line 1133: `!stderr.contains("checks for updates daily")` — still true (state exists, no privacy notice)
- Line 1139: `state.is_some()` — still true

However, the current test implicitly depends on the sync path having been exercised (the 500ms HTTP call). After this change, we should add an assertion that `last_checked` was updated to confirm the stale-path logic ran. The current timestamp `2099-01-01T00:00:00Z` means `is_stale()` returns false, but `cached.latest_version.is_none()` is true, so the background path still runs and updates `last_checked`. **Wait — actually, `update_state(|s| s.last_checked = Utc::now())` will update the timestamp, but since the original timestamp is `2099-01-01T00:00:00Z` (in the future), the new timestamp will be earlier.** The test currently doesn't check the timestamp value, so it will pass. But we should add:

```rust
// last_checked should have been updated (no longer far-future)
let state_content = read_version_state(&fake_home).unwrap();
assert!(!state_content.contains("2099-01-01"),
    "last_checked should have been updated: {}", state_content);
```

**`version_check_first_run_shows_privacy_notice`** (cli_test.rs line 1084): This test should pass unchanged — first run (no state) still shows the privacy notice and creates a state file. The difference is it now uses `spawn_background_check()` instead of the sync path, but the test only checks stderr output and state file existence, not network behavior.

**Unit tests in `version_check.rs`** (lines 284–363): All existing unit tests are for pure functions (`version_newer`, `is_stale`, `state_file_round_trip`) and don't test `check_cache_and_notify()` directly. No changes needed.

### New test to add

Add a test verifying the `latest_version: None` retry loop is broken:

```rust
#[test]
fn version_check_none_latest_version_does_not_block() {
    // Regression test: previously, latest_version=None triggered a 500ms sync HTTP call
    // on every command. Now it should just spawn a background check.
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    write_version_state(&fake_home, "2020-01-01T00:00:00Z", None);

    let start = std::time::Instant::now();
    let output = cargo_bin_cmd!("git-forest")
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();
    let elapsed = start.elapsed();

    assert!(output.status.success());
    // Should complete in well under 500ms (no sync HTTP call)
    // Use 400ms as threshold to avoid flaky timing on slow CI
    assert!(elapsed < std::time::Duration::from_millis(400),
        "version check should not block: took {:?}", elapsed);
}
```

**Note:** Timing-based tests can be flaky. An alternative is to verify that the state file's `last_checked` was updated (confirming the background path ran) without asserting timing.

---

## Change 3: Incremental output during forest deletion

### Current code structure (verified)

**`execute_reset()`** (`src/commands/reset.rs`, lines 107–170):
- Line 111–138: Iterates `plan.forests` via `.iter().map(...).collect()`, calling `std::fs::remove_dir_all(path)` at line 122 inside the map closure. Results are collected into a `Vec<ForestResetEntry>`.
- Lines 140–150: Deletes config and state files via `delete_file()`.
- Lines 152–169: Constructs and returns the full `ResetResult`.

**`cmd_reset()`** (line 220): Signature is `pub fn cmd_reset(confirm: bool, config_only: bool, dry_run: bool) -> Result<ResetResult>`. Calls `plan_reset()` then `execute_reset()`. No callback parameter.

**`format_reset_human()`** (lines 236–318): Single function producing all output. The per-forest lines are at lines 258–276. The summary (header, config, state, warnings, errors) is everything else.

**`Command::Reset` arm in `main.rs`** (lines 182–194): Calls `cmd_reset()`, then `output()` which calls `format_reset_human()`.

**`ResetPlan`** (line 38): Private struct — `struct ResetPlan` (no `pub`).

**`ForestResetEntry`** (line 30): Already `pub` with `pub` fields.

### 3a. Progress enum

Notifying *after* deletion still leaves a long silence during each `remove_dir_all`. The callback should fire *before* the slow operation ("Removing forest foo...") and *after* it completes ("done" / "FAILED"). This gives the user instant feedback that work has started, plus a trailing status.

Define a progress enum in `reset.rs`:

```rust
pub enum ResetProgress<'a> {
    ForestStarting { name: &'a str, path: &'a Path },
    ForestDone(&'a ForestResetEntry),
}
```

Target output for the incremental path:

```
Forests:
  Removing my-feature (/path/to/my-feature)... done
  Removing another-feature (/path/to/another-feature)... FAILED

Reset completed with errors.
Config: deleted (...)
State:  deleted (...)
```

The "Removing..." line is printed (without newline) on `ForestStarting`. The " done" or " FAILED" is printed (with newline) on `ForestDone`.

### 3b. Modify `execute_reset`

Change the forest removal loop from `.map().collect()` to a `for` loop with before/after callbacks:

```rust
fn execute_reset(
    plan: &ResetPlan,
    on_progress: Option<&dyn Fn(ResetProgress)>,
) -> ResetResult {
    let mut errors = Vec::new();
    let mut forest_entries = Vec::new();

    for (name, path) in &plan.forests {
        if let Some(cb) = &on_progress {
            cb(ResetProgress::ForestStarting { name, path });
        }

        let entry = if !path.exists() {
            ForestResetEntry {
                name: name.clone(),
                path: path.clone(),
                removed: false,
            }
        } else {
            match std::fs::remove_dir_all(path) {
                Ok(()) => ForestResetEntry {
                    name: name.clone(),
                    path: path.clone(),
                    removed: true,
                },
                Err(e) => {
                    errors.push(format!("failed to remove forest {}: {}", name, e));
                    ForestResetEntry {
                        name: name.clone(),
                        path: path.clone(),
                        removed: false,
                    }
                }
            }
        };

        if let Some(cb) = &on_progress {
            cb(ResetProgress::ForestDone(&entry));
        }
        forest_entries.push(entry);
    }

    // ... rest unchanged (config/state deletion, construct ResetResult)
}
```

### 3c. Thread callback through `cmd_reset`

```rust
pub fn cmd_reset(
    confirm: bool,
    config_only: bool,
    dry_run: bool,
    on_progress: Option<&dyn Fn(ResetProgress)>,
) -> Result<ResetResult> {
    let plan = plan_reset(config_only)?;

    if dry_run {
        return Ok(plan_to_dry_run(&plan));
    }

    if !confirm {
        return Ok(plan_to_confirm_required(&plan));
    }

    Ok(execute_reset(&plan, on_progress))
}
```

Note: The callback is only invoked in the `execute_reset` path (when `confirm` is true and `dry_run` is false). For dry-run and confirm-required paths, the callback is never called — these paths construct `ResetResult` from the plan directly (via `plan_to_dry_run` / `plan_to_confirm_required`) with no I/O.

### 3d. Split `format_reset_human`

Extract two new public functions:

**`format_forest_entry`** — the per-forest line for batch output (extracted from lines 258–276). Used by `format_reset_human` for dry-run/confirm-required/JSON paths:

```rust
pub fn format_forest_entry(forest: &ForestResetEntry, is_preview: bool) -> String {
    let status = if is_preview {
        if forest.removed { "would remove" } else { "already missing" }
    } else if forest.removed {
        "removed"
    } else {
        "failed"
    };
    format!("  {}: {} ({})", forest.name, status, forest.path.display())
}
```

**`format_reset_summary`** — everything except the per-forest lines:

```rust
pub fn format_reset_summary(result: &ResetResult) -> String {
    let mut lines = Vec::new();

    // Header line (lines 239-247 of current format_reset_human)
    if result.confirm_required {
        lines.push("The following would be deleted:".to_string());
    } else if result.dry_run {
        lines.push("Dry run — no changes will be made.".to_string());
    } else if result.errors.is_empty() {
        lines.push("Reset complete.".to_string());
    } else {
        lines.push("Reset completed with errors.".to_string());
    }

    // Config/state status (lines 280-294)
    lines.push(String::new());
    let is_preview = result.dry_run || result.confirm_required;
    let config_status = format_file_status(&result.config_file, is_preview);
    lines.push(format!("Config: {} ({})", config_status, result.config_file.path.display()));
    let state_status = format_file_status(&result.state_file, is_preview);
    lines.push(format!("State:  {} ({})", state_status, result.state_file.path.display()));

    // Warnings (lines 296-302)
    if !result.warnings.is_empty() { ... }

    // Errors (lines 304-310)
    if !result.errors.is_empty() { ... }

    // Confirm hint (lines 312-315)
    if result.confirm_required { ... }

    lines.join("\n")
}
```

**Keep `format_reset_human` for batch paths** — used by `output()` which needs `fn(&T) -> String`. Rewrite it to delegate to the two new functions:

```rust
pub fn format_reset_human(result: &ResetResult) -> String {
    let mut parts = Vec::new();
    let is_preview = result.dry_run || result.confirm_required;

    // Header first (matches current output order)
    parts.push(format_reset_summary(result));

    // Per-forest lines
    if !result.config_only {
        if result.forests.is_empty() {
            parts.push("\nForests: none found".to_string());
        } else {
            parts.push("\nForests:".to_string());
            for forest in &result.forests {
                parts.push(format_forest_entry(forest, is_preview));
            }
        }
    }

    parts.join("\n")
}
```

Wait — this changes the output order for batch paths too (summary before forests). The current order is: header → forests → config/state → warnings → errors → confirm hint. To preserve that for batch and only change the incremental path, `format_reset_human` should keep its current implementation and just call the helpers internally. The summary/forest split only matters for the incremental path in `main.rs`.

Simpler approach: **don't refactor `format_reset_human` at all.** Keep it exactly as-is for batch output (JSON, dry-run, confirm-required). The new `format_reset_summary` and `format_forest_entry` are only used by the incremental path in `main.rs`.

### 3e. Restructure `Command::Reset` arm in `main.rs`

```rust
Command::Reset { confirm, config_only, dry_run } => {
    let result = if cli.json {
        let r = commands::cmd_reset(confirm, config_only, dry_run, None)?;
        output(&r, true, commands::format_reset_human)?;
        r
    } else if dry_run || !confirm {
        // Non-destructive paths: batch output as before
        let r = commands::cmd_reset(confirm, config_only, dry_run, None)?;
        let text = commands::format_reset_human(&r);
        if !text.is_empty() { println!("{}", text); }
        r
    } else {
        // Destructive path: incremental output with leading+trailing
        use std::io::Write;
        if !config_only {
            println!("Forests:");
        }
        let r = commands::cmd_reset(confirm, config_only, dry_run, Some(&|progress| {
            match progress {
                commands::ResetProgress::ForestStarting { name, path } => {
                    print!("  Removing {} ({})...", name, path.display());
                    std::io::stdout().flush().ok();
                }
                commands::ResetProgress::ForestDone(entry) => {
                    if entry.removed {
                        println!(" done");
                    } else {
                        println!(" FAILED");
                    }
                }
            }
        }))?;
        println!("{}", commands::format_reset_summary(&r));
        r
    };
    if !result.errors.is_empty() || result.confirm_required {
        std::process::exit(1);
    }
}
```

Note the `flush()` after `print!` — without it, the "Removing..." text may be buffered and not appear until the newline in `ForestDone`. This is critical for the leading-message UX.

### 3f. Tests

**Existing tests:** All existing unit tests in `reset.rs` (lines 337–808) call `cmd_reset()` directly. They all need a `None` callback parameter added to their `cmd_reset()` calls. Affected lines:
- Line 520: `cmd_reset(true, false, false)` → `cmd_reset(true, false, false, None)`
- Line 563: `cmd_reset(true, true, false)` → `cmd_reset(true, true, false, None)`
- Line 605: `cmd_reset(false, false, false)` → `cmd_reset(false, false, false, None)`
- Line 644: `cmd_reset(false, false, true)` → `cmd_reset(false, false, true, None)`
- Line 700: `cmd_reset(true, false, false)` → `cmd_reset(true, false, false, None)`
- Line 759: `cmd_reset(true, true, false)` → `cmd_reset(true, true, false, None)`
- Line 791: `cmd_reset(true, false, false)` → `cmd_reset(true, false, false, None)`

**New test — callback fires Starting then Done for each forest:**

```rust
#[test]
fn execute_reset_fires_progress_starting_then_done() {
    // Setup: create tmp env with a forest to remove...
    let events = std::cell::RefCell::new(Vec::new());
    let result = cmd_reset(true, false, false, Some(&|progress| {
        match progress {
            ResetProgress::ForestStarting { name, .. } => {
                events.borrow_mut().push(format!("start:{}", name));
            }
            ResetProgress::ForestDone(entry) => {
                events.borrow_mut().push(format!("done:{}", entry.name));
            }
        }
    })).unwrap();

    // Should see start/done pairs in order
    let events = events.borrow();
    assert_eq!(events.len(), result.forests.len() * 2);
    for (i, forest) in result.forests.iter().enumerate() {
        assert_eq!(events[i * 2], format!("start:{}", forest.name));
        assert_eq!(events[i * 2 + 1], format!("done:{}", forest.name));
    }
}
```

---

## Change 4: Optimize the Cloudflare Worker

**File:** `worker/src/index.ts` (44 lines), `worker/wrangler.toml` (18 lines)

### Current code structure (verified)

**`worker/src/index.ts`:**
- Line 3–6: `Env` interface has `DB: D1Database` and `KV: KVNamespace`.
- Line 9: Handler signature is `async fetch(request: Request, env: Env): Promise<Response>` — **`ctx` (ExecutionContext) is NOT in the signature**. Must be added.
- Lines 27–35: D1 insert wrapped in try/catch with `await` — blocks response path.
- Lines 38–41: `const latest = await env.KV.get("latest_version")` — KV read, also blocks. Fallback hardcoded: `latest || "0.2.3"` (stale — current version is 0.2.14).

**`worker/wrangler.toml`:**
- Lines 11–14: D1 binding (`[[d1_databases]]`)
- Lines 16–18: KV binding (`[[kv_namespaces]]`, id `12a2af8dcfaa43b0a433cc1e8c459ec9`)
- No `[vars]` section currently

### 4a. Add `ctx` parameter and move D1 write to `waitUntil()`

The handler signature must change from 2-arg to 3-arg:

```typescript
// Before (line 9):
async fetch(request: Request, env: Env): Promise<Response> {

// After:
async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
```

Replace the try/catch D1 block (lines 27–35) with:

```typescript
ctx.waitUntil(
    env.DB.prepare(
        "INSERT INTO events (city, country, version, timestamp) VALUES (?, ?, ?, ?)",
    )
        .bind(city, country, version, timestamp)
        .run()
        .catch((e) => console.error("D1 write failed:", e))
);
```

`ExecutionContext` is a global type in the Cloudflare Workers runtime — no import needed.

### 4b. Replace KV with `[vars]`

**`worker/src/index.ts`:**

Replace `Env` interface (lines 3–6):
```typescript
// Before:
interface Env {
  DB: D1Database;
  KV: KVNamespace;
}

// After:
interface Env {
  DB: D1Database;
  LATEST_VERSION: string;
}
```

Replace KV read (lines 38–41):
```typescript
// Before:
const latest = await env.KV.get("latest_version");
const response: VersionResponse = {
    version: latest || "0.2.3",
};

// After:
const response: VersionResponse = {
    version: env.LATEST_VERSION,
};
```

**`worker/wrangler.toml`:**

Delete the KV block (lines 16–18):
```toml
[[kv_namespaces]]
binding = "KV"
id = "12a2af8dcfaa43b0a433cc1e8c459ec9"
```

Add a `[vars]` section (after the D1 block, or at end):
```toml
[vars]
LATEST_VERSION = "0.2.14"
```

### 4c. Remove KV recipes from justfile

Delete these two recipes (justfile lines 34–38):
```
worker-kv-create:
    cd worker && npx wrangler kv namespace create GIT_FOREST_KV

worker-kv-seed:
    cd worker && npx wrangler kv key put --remote --binding=KV latest_version "0.2.3"
```

These are dead code once KV is removed. The `worker-kv-seed` recipe also has a stale version ("0.2.3").

### Complete resulting `worker/src/index.ts`:

```typescript
import type { VersionResponse } from "./generated/VersionResponse";

interface Env {
  DB: D1Database;
  LATEST_VERSION: string;
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname !== "/api/latest") {
      return new Response("Not found", { status: 404 });
    }

    if (request.method !== "GET") {
      return new Response("Method not allowed", { status: 405 });
    }

    const version = url.searchParams.get("v") || "unknown";
    const cf = (request as any).cf || {};
    const city = cf.city || null;
    const country = cf.country || null;
    const timestamp = new Date().toISOString();

    ctx.waitUntil(
      env.DB.prepare(
        "INSERT INTO events (city, country, version, timestamp) VALUES (?, ?, ?, ?)",
      )
        .bind(city, country, version, timestamp)
        .run()
        .catch((e) => console.error("D1 write failed:", e))
    );

    const response: VersionResponse = {
      version: env.LATEST_VERSION,
    };
    return Response.json(response);
  },
};
```

---

## Change 5: `just release` workflow

**File:** `justfile` (currently 44 lines)

### Current state (verified)

- No existing `release` recipe or version automation.
- Existing worker recipes: `worker-deploy` (line 26), `worker-db-create` (line 28), `worker-db-migrate` (line 31), `worker-kv-create` (line 34), `worker-kv-seed` (line 37), `worker-logs` (line 40), `worker-query` (line 43).
- Current Cargo.toml version: `0.2.14` (line 3).
- CLAUDE.md says: "Always push the tag by name — `git push origin v0.x.y` — not `--tags`."

### Recipe

Add after the `loc` recipe (line 19, before the worker section):

```just
# macOS-only (sed -i '' syntax). Bumps version, commits, tags, pushes, deploys worker.
release version:
    #!/usr/bin/env bash
    set -euo pipefail

    # 1. Verify clean state and passing checks
    just check
    just test

    # 2. Update version in Cargo.toml and wrangler.toml
    sed -i '' 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    sed -i '' 's/^LATEST_VERSION = ".*"/LATEST_VERSION = "{{version}}"/' worker/wrangler.toml

    # 3. Rebuild to update Cargo.lock
    cargo check
    just check

    # 4. Commit, tag, push (push tag by name per CLAUDE.md)
    git add Cargo.toml Cargo.lock worker/wrangler.toml
    git commit -m "chore: bump version to {{version}}"
    git tag "v{{version}}"
    git push
    git push origin "v{{version}}"

    # 5. Deploy worker with new LATEST_VERSION
    just worker-deploy

    echo "Released v{{version}}"
```

Note: The `sed -i ''` for `worker/wrangler.toml` depends on Change 4 having added the `LATEST_VERSION` var. If Change 4 hasn't been applied yet, this recipe won't find the pattern to replace. This is fine — Change 5 depends on Change 4 per the ordering section.

### Also remove the two dead KV recipes

(Same deletion as Change 4c — listed here for completeness since justfile is the file being edited.)

---

## Change 6: Incremental output for `rm` (same pattern as Change 3)

### Current code structure (verified)

**`execute_rm()`** (`src/commands/rm.rs`, lines 83–110):
- Lines 87–97: `for` loop over `plan.repo_plans`, calling `remove_worktree()` and `delete_branch()`. Results are pushed to `repos: Vec<RepoRmResult>`. **Already uses a `for` loop** (not `.map().collect()` like `execute_reset`), so adding a callback is straightforward.
- Line 99: `remove_forest_dir()` after all repos processed.

**`cmd_rm()`** (lines 348–361): Signature is `pub fn cmd_rm(forest_dir: &Path, meta: &ForestMeta, force: bool, dry_run: bool) -> Result<RmResult>`. No callback parameter.

**`format_rm_human()`** (lines 363–433): Per-repo lines at lines 382–413. Summary is the header (lines 366–380), forest dir status (lines 416–422), and errors (lines 424–430).

**`Command::Rm` arm in `main.rs`** (lines 167–181): Calls `cmd_rm()`, then `output()`.

### 6a. Progress enum

Same leading+trailing pattern as Change 3. For `rm`, the slow operations are per-repo (`git worktree remove` and potentially `remove_dir_all` for missing-source fallback), not per-forest.

```rust
pub enum RmProgress<'a> {
    RepoStarting { name: &'a RepoName },
    RepoDone(&'a RepoRmResult),
}
```

Target output for the incremental path:

```
Removing forest "my-feature"
  foo-api: removing... worktree removed, branch deleted
  foo-web: removing... worktree removed (branch not ours)
Forest directory removed.
```

The "removing..." text is printed (without newline) on `RepoStarting`. The trailing status is printed (with newline) on `RepoDone`.

### 6b. Modify `execute_rm`

The loop at lines 87–97 already uses `for` — just add the callback calls around the existing work:

```rust
pub fn execute_rm(
    plan: &RmPlan,
    force: bool,
    on_progress: Option<&dyn Fn(RmProgress)>,
) -> RmResult {
    let mut repos = Vec::new();
    let mut errors = Vec::new();

    for repo_plan in &plan.repo_plans {
        if let Some(cb) = &on_progress {
            cb(RmProgress::RepoStarting { name: &repo_plan.name });
        }

        let (worktree_removed, wt_succeeded) = remove_worktree(repo_plan, force, &mut errors);
        let branch_deleted = delete_branch(repo_plan, force, wt_succeeded, &mut errors);

        let result = RepoRmResult {
            name: repo_plan.name.clone(),
            worktree_removed,
            branch_deleted,
        };

        if let Some(cb) = &on_progress {
            cb(RmProgress::RepoDone(&result));
        }
        repos.push(result);
    }

    let forest_dir_removed = remove_forest_dir(&plan.forest_dir, force, &mut errors);

    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run: false,
        force,
        repos,
        forest_dir_removed,
        errors,
    }
}
```

### 6c. Thread callback through `cmd_rm`

```rust
pub fn cmd_rm(
    forest_dir: &std::path::Path,
    meta: &ForestMeta,
    force: bool,
    dry_run: bool,
    on_progress: Option<&dyn Fn(RmProgress)>,
) -> Result<RmResult> {
    let plan = plan_rm(forest_dir, meta);
    if dry_run {
        return Ok(plan_to_dry_run_result(&plan));
    }
    Ok(execute_rm(&plan, force, on_progress))
}
```

### 6d. Format helpers

**`format_repo_done`** — trailing status for a completed repo (extracted from the per-repo format logic at lines 382–413):

```rust
pub fn format_repo_done(repo: &RepoRmResult) -> String {
    let wt = match &repo.worktree_removed {
        RmOutcome::Success => "worktree removed".to_string(),
        RmOutcome::Skipped { reason } => format!("worktree skipped ({})", reason),
        RmOutcome::Failed { .. } => "worktree FAILED".to_string(),
    };

    let br = match &repo.branch_deleted {
        RmOutcome::Success => ", branch deleted".to_string(),
        RmOutcome::Skipped { reason } => {
            if reason == "branch not created by forest" {
                " (branch not ours)".to_string()
            } else {
                format!(", branch skipped ({})", reason)
            }
        }
        RmOutcome::Failed { .. } => ", branch FAILED".to_string(),
    };

    format!("{}{}", wt, br)
}
```

**`format_rm_summary`** — forest dir status + errors (everything except per-repo lines):

```rust
pub fn format_rm_summary(result: &RmResult) -> String {
    let mut lines = Vec::new();

    // Forest dir status (lines 416-422)
    if result.forest_dir_removed {
        lines.push("Forest directory removed.".to_string());
    } else {
        lines.push("Forest directory not removed (not empty).".to_string());
    }

    // Errors (lines 424-430)
    if !result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in &result.errors {
            lines.push(format!("  {}", error));
        }
    }

    lines.join("\n")
}
```

**Keep `format_rm_human` unchanged** for batch paths (JSON, dry-run). No refactoring needed — same rationale as Change 3.

### 6e. Restructure `Command::Rm` arm in `main.rs`

```rust
Command::Rm { name, force, dry_run } => {
    let config = config::load_default_config()?;
    let bases = config.all_worktree_bases();
    let (dir, meta) = forest::resolve_forest_multi(&bases, name.as_deref())?;
    let result = if cli.json || dry_run {
        let r = commands::cmd_rm(&dir, &meta, force, dry_run, None)?;
        output(&r, cli.json, commands::format_rm_human)?;
        r
    } else {
        use std::io::Write;
        println!("Removing forest {:?}", meta.name.as_str());
        let r = commands::cmd_rm(&dir, &meta, force, dry_run, Some(&|progress| {
            match progress {
                commands::RmProgress::RepoStarting { name } => {
                    print!("  {}: removing...", name);
                    std::io::stdout().flush().ok();
                }
                commands::RmProgress::RepoDone(repo) => {
                    println!(" {}", commands::format_repo_done(repo));
                }
            }
        }))?;
        println!("{}", commands::format_rm_summary(&r));
        r
    };
    if !result.errors.is_empty() {
        std::process::exit(1);
    }
}
```

Note the `flush()` after `print!` — same as Change 3, required so the "removing..." text appears before the slow operation.

### 6f. Tests

**Existing tests in `rm.rs`** (lines 436–1140): All call `cmd_rm(forest_dir, meta, force, dry_run)`. Each needs a `None` callback parameter added. Affected calls (there are many — roughly 15 call sites in the test functions):
- Line 554, 578, 606, 635, 655, 678, 709, 738, 767, 796, 824, 845, 856, 956, 996, 1032, 1063, 1101, 1131

**New test — callback fires Starting then Done for each repo:**

```rust
#[test]
fn execute_rm_fires_progress_starting_then_done() {
    let env = TestEnv::new();
    env.create_repo_with_remote("foo-api");
    env.create_repo_with_remote("foo-web");
    let tmpl = env.default_template(&["foo-api", "foo-web"]);

    let inputs = make_new_inputs("rm-progress", ForestMode::Feature);
    cmd_new(inputs, &tmpl).unwrap();

    let forest_dir = tmpl.worktree_base.join("rm-progress");
    let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

    let events = std::cell::RefCell::new(Vec::new());
    let result = cmd_rm(&forest_dir, &meta, false, false, Some(&|progress| {
        match progress {
            RmProgress::RepoStarting { name } => {
                events.borrow_mut().push(format!("start:{}", name));
            }
            RmProgress::RepoDone(repo) => {
                events.borrow_mut().push(format!("done:{}", repo.name));
            }
        }
    })).unwrap();

    let events = events.borrow();
    assert_eq!(events.len(), result.repos.len() * 2);
    for (i, repo) in result.repos.iter().enumerate() {
        assert_eq!(events[i * 2], format!("start:{}", repo.name));
        assert_eq!(events[i * 2 + 1], format!("done:{}", repo.name));
    }
}
```

---

## Ordering

1. **Change 1** — one-line removal, biggest immediate impact
2. **Change 2** — small refactor, eliminates the only sync HTTP call in the CLI
3. **Change 4** — server-side, independent of CLI changes, improves cold-start for all users
4. **Change 5** — workflow improvement, depends on Change 4 (wrangler.toml `LATEST_VERSION` var)
5. **Change 3** — largest refactor, most user-visible improvement for `reset` specifically
6. **Change 6** — same pattern as Change 3, apply to `rm`

Changes 1–2 (CLI) and 4 (server) can be done in parallel. Change 5 depends on 4. Changes 3 and 6 are independent of each other but both touch `main.rs`, so doing them sequentially reduces merge conflicts.

## Verification

```
just check      # fmt + clippy
just test       # all tests (includes unit + integration)
just test-linux # cross-platform (Docker)
```
