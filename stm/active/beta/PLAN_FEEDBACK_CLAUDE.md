# Plan Feedback — Claude Code Review

## Overall assessment

The design is thorough and well-structured. The approach (Cargo features + `[[bin]]`) is the right call for exactly two known channels. The plan's step ordering is logical — constants module first, wire into consumers, then CI/worker/homebrew. A few things I'd flag:

## Issues

### 1. `just release` no longer updates the right wrangler.toml var

The plan's step 6 changes `LATEST_VERSION` → `LATEST_VERSION_STABLE` + `LATEST_VERSION_BETA` in wrangler.toml. But the existing `just release` recipe (justfile line 31) does:
```
sed -i '' 's/^LATEST_VERSION = ".*"/LATEST_VERSION = "{{version}}"/' worker/wrangler.toml
```
That sed pattern won't match `LATEST_VERSION_STABLE`. Step 9 mentions adding `just release-beta` but doesn't say to update the existing `just release` to target `LATEST_VERSION_STABLE`. The plan should be explicit: update the sed in `just release` to match `LATEST_VERSION_STABLE`, and `just release-beta` targets `LATEST_VERSION_BETA`.

### 2. `brew upgrade git-forest` is hardcoded in `Command::Update`

`main.rs:300-313` — the `Update` command hardcodes `brew --prefix git-forest` and `brew upgrade git-forest`. With the beta binary, `git forest-beta update` would try to upgrade the `git-forest` formula, not `git-forest-beta`. This needs to use `channel::APP_NAME`. The plan doesn't mention `Command::Update` at all.

### 3. `version` subcommand output is hardcoded

`main.rs:277` — `println!("git-forest {}", ...)` and the update notice messages on lines 285-286 both hardcode `"git-forest"`. Same for `version_check.rs:218` (`"Update available: git-forest v{}"`) and line 201 (`"Note: git-forest checks for updates daily..."`). The plan covers the privacy notice (step 4) but doesn't explicitly list the `Command::Version` branch in main.rs or the update notice in `check_cache_and_notify`.

### 4. User-Agent header is hardcoded

`version_check.rs:106` — `.header("User-Agent", &format!("git-forest/{}", current))`. Should be `channel::APP_NAME` for accurate analytics. Not mentioned in the plan.

### 5. `compile_error!` + `cargo test` default behavior

The plan notes `default = ["stable"]` means `cargo test` works, and `--all-features` hits the `compile_error!`. But `cargo clippy --all-targets` (in `just check`, justfile line 5) doesn't specify features — it'll use the default, which is fine. However, the plan's step 8 changes CI to test both feature sets explicitly but doesn't update `just check` / `just test` locally. Should the justfile recipes also run both, or is default-only fine for local dev? Worth being explicit.

### 6. `--internal-version-check` is shared, not per-channel — fine?

The plan makes `INTERNAL_VERSION_CHECK_ARG` shared (`"--internal-version-check"` for both channels). Since `current_exe()` already returns the right binary, this is correct — the same arg name is fine. Just confirming the plan is right here.

## Open questions feedback

- **D1 schema**: Agree, explicit `channel TEXT` column is cleaner. Version string parsing is fragile.
- **`sed` vs `toml-cli`**: Given there are now two similar vars, a `toml set` approach would be safer than sed matching on prefixes. But sed works if the patterns are distinct enough (`LATEST_VERSION_STABLE` vs `LATEST_VERSION_BETA`).
- **Worker backward compat**: No need to keep `LATEST_VERSION` as an alias — old clients don't send `?channel=`, worker defaults to stable, reads `LATEST_VERSION_STABLE`. Clean cut.
- **Beta → stable promotion**: New stable tag at the same (or cherry-picked) commit. No special mechanism.

## Summary

The architecture is sound. The main gaps are items 2 and 3 above — `Command::Update` and `Command::Version` in main.rs aren't covered by the plan but have hardcoded `"git-forest"` strings that need channelization. Item 1 (sed pattern) is a mechanical bug that would break the first stable release after the change.
