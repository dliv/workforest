# Reset Performance Improvements

## Problem

`git forest reset --confirm` appears slow with no output until completion. All output arrives at once after several seconds of silence.

## Root Causes

### 1. No incremental output during forest deletion (primary)

`execute_reset()` in `src/commands/reset.rs` (line 107) does all `remove_dir_all()` calls in a batch via `.map().collect()`, building a `Vec<ForestResetEntry>`. The entire `ResetResult` is returned to `main.rs`, which passes it to `output()` → `format_reset_human()`. No output is printed until every forest directory is fully deleted.

When forests contain node_modules, .git objects, JARs, etc., individual `remove_dir_all()` calls can take seconds each. With multiple forests this compounds.

**Key code path:**
- `main.rs:187` — `cmd_reset(confirm, config_only, dry_run)?`
- `main.rs:190` — `output(&result, cli.json, commands::format_reset_human)?` (all output happens here, after all work)
- `reset.rs:122` — `std::fs::remove_dir_all(path)` (the slow part, inside a `.map()`)

**Proposed fix:** Print incrementally during execution. The plan/execute split (ADR 0003) protects plan purity — the plan step stays pure data. Execution is already the imperative shell (`remove_dir_all`, `remove_file`), so streaming output during execution doesn't violate any architectural invariant. The current "collect everything, return, print at the end" pattern in `main.rs:output()` is a convenience, not a constraint.

Options:
- Pass a callback/writer to `execute_reset` for progress reporting
- Print to stderr during execution, keep stdout for final summary
- Pass `json: bool` to `cmd_reset` and print stdout incrementally when not JSON
- Streaming JSON (NDJSON or incremental array) — also valid since execution is already impure, though probably not practical for this case

`cmd_rm` in `rm.rs` has the same pattern (batch then print) but it only removes one forest at a time, so it's less noticeable.

### 2. Version check blocks after reset deletes state file

After `reset --confirm` deletes the state file, the version check at `main.rs:44-46` (`check_cache_and_notify`) finds no cached state and enters the synchronous `needs_sync` path (`version_check.rs:197`), which calls `fetch_latest_version` with a 500ms timeout.

Additionally, the server (`forest.dliv.gg`) has Cloudflare Worker cold-start latency of ~1s. The 500ms timeout is too short for cold starts — it times out, writes `latest_version: None`, and the *next* command hits the same `needs_sync` path again (because `latest_version` is `None` at line 194). This creates a repeating cycle of 500ms blocking until the server responds within 500ms.

**Measured latency:**
- DNS: ~36ms (fast)
- TCP connect: ~57ms (fast)
- Server TTFB (cold): ~965ms (exceeds 500ms timeout)
- Server TTFB (warm): ~150ms (within timeout)

**Proposed fix:** Remove `Command::Reset` from the `should_version_check` match in `main.rs:28-37`. Reset deletes state, so running a version check after it is counterproductive.

**Broader fix:** Consider whether the synchronous first-run path (`needs_sync`) should exist at all — it could spawn the background subprocess instead of blocking.

## Testing

Need perf-oriented tests:
- Unit test that `execute_reset` with large directory trees produces output incrementally (once incremental output is implemented)
- Test that version check is skipped after `reset --confirm`
- Test the `needs_sync` retry loop: when `latest_version` is `None` in state, verify it doesn't block on every subsequent command

## Files Involved

- `src/main.rs` — command dispatch, version check gating (lines 28-46)
- `src/commands/reset.rs` — `execute_reset()` (line 107), `format_reset_human()` (line 236)
- `src/version_check.rs` — `check_cache_and_notify()` (line 179), `fetch_latest_version()` (line 92)

## Status

Investigation complete. Implementation not started.
