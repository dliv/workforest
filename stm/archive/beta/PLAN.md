# Plan: Beta release infrastructure

## Overview

Add compile-time channel switching so the same codebase produces `git-forest` (stable) and `git-forest-beta` (beta) binaries with separate config/state directories, independent version checking, and coexisting Homebrew formulae. See [DESIGN.md](DESIGN.md) for full rationale.

## Step 1: Add `src/channel.rs` — channel constants module

**File:** `src/channel.rs` (new)

Centralize all per-channel constants behind Cargo feature gates:

- `APP_NAME`: `"git-forest"` (stable) / `"git-forest-beta"` (beta) — used for XDG dirs
- `VERSION_CHANNEL`: `"stable"` / `"beta"` — sent as query param in version check
- `VERSION_CHECK_BASE_URL`: `"https://forest.dliv.gg/api/latest"` — shared, not per-channel
- `INTERNAL_VERSION_CHECK_ARG`: `"--internal-version-check"` — DRYs up the duplicated string

Add `compile_error!` guard for mutually exclusive features.

**No callers yet** — wired up in subsequent steps.

## Step 2: Update `Cargo.toml` — features and binary targets

**File:** `Cargo.toml`

Add:

```toml
[features]
default = ["stable"]
stable = []
beta = []

[[bin]]
name = "git-forest"
path = "src/main.rs"
required-features = ["stable"]

[[bin]]
name = "git-forest-beta"
path = "src/main.rs"
required-features = ["beta"]
```

Remove the implicit `[[bin]]` that Cargo derives from `[package] name`. The `[package] name` stays `"git-forest"` (it's the crate name, not the binary name).

**Verify:** `cargo build --no-default-features --features stable` and `cargo build --no-default-features --features beta` both compile. `cargo build --all-features` hits the `compile_error!`.

## Step 3: Wire `channel::APP_NAME` into `src/config.rs`

**File:** `src/config.rs`

- Remove `const APP_NAME: &str = "git-forest";` (line 102)
- Add `use crate::channel;`
- Replace all uses of `APP_NAME` with `channel::APP_NAME`

The XDG helpers (`xdg_config_dir`, `xdg_state_dir`) and any error messages referencing the app name now use the channel-aware constant.

**Verify:** `cargo test --no-default-features --features stable` — existing config tests pass. Config paths still resolve to `git-forest/`. Then `cargo test --no-default-features --features beta` — paths resolve to `git-forest-beta/`.

Note: the `xdg_dir_with_env` tests hardcode expected paths like `"/tmp/test-xdg-config/git-forest"`. These need to become channel-aware (e.g., use `channel::APP_NAME` in the assertion, or accept that the test is only valid for one feature). Simplest: assert the path ends with `channel::APP_NAME` instead of a literal string.

## Step 4: Wire `channel::*` into `src/version_check.rs`

**File:** `src/version_check.rs`

- Remove `const VERSION_CHECK_URL` (line 7)
- Add `use crate::channel;`
- In `fetch_latest_version`: build URL as `format!("{}?v={}&channel={}", channel::VERSION_CHECK_BASE_URL, current, channel::VERSION_CHANNEL)`
- In `spawn_background_check`: replace `"--internal-version-check"` literal with `channel::INTERNAL_VERSION_CHECK_ARG`
- In `check_cache_and_notify` privacy notice (line 201): replace hardcoded `"git-forest"` with `channel::APP_NAME`
- In `check_cache_and_notify` update notice (line 218): replace hardcoded `"git-forest"` with `channel::APP_NAME`
- User-Agent header (line 106): replace `"git-forest/{}"` with `format!("{}/{}", channel::APP_NAME, current)`

**Verify:** `cargo test` — version check unit tests pass.

## Step 5: Wire `channel::*` into `src/main.rs`

**File:** `src/main.rs`

- Add `mod channel;`
- Replace `"--internal-version-check"` literal (line 20) with `channel::INTERNAL_VERSION_CHECK_ARG`
- `Command::Version` (line 277): replace `"git-forest {}"` with `format!("{} {}", channel::APP_NAME, ...)`
- `Command::Version` update notice (lines 285-286): replace hardcoded `"git-forest"` with `channel::APP_NAME`
- `Command::Update` (lines 300-313): replace hardcoded `"git-forest"` in `brew --prefix git-forest` and `brew upgrade git-forest` with `channel::APP_NAME`

**Verify:** `cargo test --no-default-features --features stable` and `--features beta` both pass. Full `just check` passes.

## Step 6: Update worker to support multi-channel versions

**Files:** `worker/wrangler.toml`, `worker/src/index.ts`

### wrangler.toml

Replace:
```toml
[vars]
LATEST_VERSION = "0.2.17"
```

With:
```toml
[vars]
LATEST_VERSION_STABLE = "0.2.17"
LATEST_VERSION_BETA = "0.0.0"
```

### index.ts

- Update `Env` interface: replace `LATEST_VERSION` with `LATEST_VERSION_STABLE` and `LATEST_VERSION_BETA`
- Read `channel` query param (default `"stable"`)
- Select version: `channel === "beta" ? env.LATEST_VERSION_BETA : env.LATEST_VERSION_STABLE`
- Add `channel` to D1 insert (requires schema migration — add `channel TEXT` column)

### D1 schema

Add migration: `ALTER TABLE events ADD COLUMN channel TEXT;`

**Verify:** `cd worker && npx wrangler dev` — test both `?channel=stable` and `?channel=beta` locally.

## Step 7: Update CI — release workflow

**File:** `.github/workflows/release.yml`

Add channel detection step at the top of the `build` job:

```yaml
- name: Determine channel
  id: channel
  run: |
    if [[ "$GITHUB_REF_NAME" == *-beta* ]]; then
      echo "channel=beta" >> "$GITHUB_OUTPUT"
      echo "bin=git-forest-beta" >> "$GITHUB_OUTPUT"
      echo "features=beta" >> "$GITHUB_OUTPUT"
    else
      echo "channel=stable" >> "$GITHUB_OUTPUT"
      echo "bin=git-forest" >> "$GITHUB_OUTPUT"
      echo "features=stable" >> "$GITHUB_OUTPUT"
    fi
```

Update build step:
```yaml
cargo build --release --target ${{ matrix.target }} \
  --no-default-features --features ${{ steps.channel.outputs.features }} \
  --bin ${{ steps.channel.outputs.bin }}
```

Update package step to use `${{ steps.channel.outputs.bin }}` for artifact naming.

Update `update-homebrew` job to conditionally update `git-forest.rb` or `git-forest-beta.rb` based on channel output.

## Step 8: Update CI — test both features

**File:** `.github/workflows/ci.yml`

Replace the single `cargo test` step with:
```yaml
- run: cargo test --no-default-features --features stable
- run: cargo test --no-default-features --features beta
```

Also update clippy to run both:
```yaml
- run: cargo clippy --no-default-features --features stable --all-targets -- -D warnings
- run: cargo clippy --no-default-features --features beta --all-targets -- -D warnings
```

## Step 9: Update `just release` and add `just release-beta`

**File:** `justfile`

Update existing `release` recipe:
- Change sed target from `LATEST_VERSION` to `LATEST_VERSION_STABLE` (line 31 currently: `s/^LATEST_VERSION = /.../ ` → `s/^LATEST_VERSION_STABLE = /.../ `)

Add a `release-beta` recipe that mirrors `release` but:
- Sed targets `LATEST_VERSION_BETA` in `wrangler.toml` (not `LATEST_VERSION_STABLE`)
- Tags with the version as-is (e.g., `v0.3.0-beta.1`)
- Cargo.toml version is set to the full pre-release version (e.g., `0.3.0-beta.1`)

## Step 10: Add beta Homebrew formula

**Repo:** `dliv/homebrew-tools`

Add `Formula/git-forest-beta.rb` — same structure as `git-forest.rb` but:
- `name "git-forest-beta"`
- Downloads beta-tagged release artifacts (named `git-forest-beta-*`)
- No `conflicts_with` — both installable simultaneously

## Files changed

| File | Changes |
|---|---|
| `src/channel.rs` | **New.** Channel constants module |
| `Cargo.toml` | `[features]`, `[[bin]]` sections |
| `src/main.rs` | `mod channel`, use shared constant for internal arg, channelize `Version` and `Update` commands |
| `src/config.rs` | Remove local `APP_NAME`, use `channel::APP_NAME` |
| `src/version_check.rs` | Use `channel::*` for URL, channel param, subprocess arg, privacy notice, update notice, User-Agent |
| `worker/wrangler.toml` | Two version vars |
| `worker/src/index.ts` | Multi-channel version lookup |
| `.github/workflows/release.yml` | Channel detection, parameterized build/package/homebrew |
| `.github/workflows/ci.yml` | Test + clippy for both features |
| `justfile` | Update `release` sed target, add `release-beta` recipe |
| `dliv/homebrew-tools` | New `git-forest-beta.rb` formula |

## Edge cases

- **Old clients** (pre-channel) don't send `?channel=` → worker defaults to stable. Correct.
- **`cargo test` without feature flags** → uses `default = ["stable"]`, tests run as stable. Fine for local dev.
- **`cargo build` without `--bin`** → builds whichever binary matches default features (`git-forest`). Fine for local dev.
- **Beta → stable promotion** → new stable tag at same (or later) commit. No special mechanism needed.
- **`current_exe()` in subprocess spawn** → returns the running binary (`git-forest` or `git-forest-beta`), so background version check stays in the correct channel automatically.

## Not changing

- No new dependencies
- `is_enabled()` logic stays the same
- The set of commands that trigger version check stays the same
- ADR structure, existing tests (except path assertion adjustments in Step 3)
- `just check` / `just test` — continue to use default features (stable) for local dev; CI covers both
