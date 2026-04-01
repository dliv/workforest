# Beta Release Infrastructure — Design

## Goal

Enable beta (`git-forest-beta`) and stable (`git-forest`) to coexist on the same machine with independent config/state, version checking, and Homebrew formulae. Single Cloudflare Worker serves both channels. Build-time constants bake channel identity into each executable.

## Current Architecture (what changes)

| Component | Current | Location |
|---|---|---|
| Config/state dir name | `const APP_NAME: &str = "git-forest"` | `src/config.rs:102` |
| Version check URL | `const VERSION_CHECK_URL: &str = "https://forest.dliv.gg/api/latest"` | `src/version_check.rs:7` |
| Privacy notice text | Hardcoded `"git-forest"` and `"forest.dliv.gg"` | `src/version_check.rs:200-203` |
| Internal subprocess flag | `"--internal-version-check"` duplicated | `src/main.rs:20`, `src/version_check.rs:141` |
| Binary name | `[package] name = "git-forest"` in Cargo.toml | `Cargo.toml:2` |
| Worker latest version | `LATEST_VERSION = "0.2.17"` (single value) | `worker/wrangler.toml:17` |
| CI release trigger | `tags: ["v*"]` (no channel awareness) | `.github/workflows/release.yml:5` |
| Homebrew | Single formula `git-forest.rb` | `dliv/homebrew-tools` |

## Decisions

### 1. Compile-time switching: Cargo features + `[[bin]]`

**Chosen over** `build.rs` and `env!()`/`option_env!()`.

Reasoning:
- **`env!()`** alone doesn't trigger rebuilds when the var changes — you'd need `build.rs` anyway just to add `cargo:rerun-if-env-changed`, at which point it becomes the `build.rs` approach.
- **`build.rs`** is more moving parts (generated file, rerun directives) for no gain when there are exactly two known channels.
- **Cargo features** are idiomatic, deterministic, cache-friendly, and work natively with `cargo install` (which Homebrew uses for source builds).

```toml
# Cargo.toml additions
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

New `src/channel.rs` centralizes all per-channel constants:

```rust
#[cfg(all(feature = "stable", feature = "beta"))]
compile_error!("features 'stable' and 'beta' are mutually exclusive");

#[cfg(feature = "stable")]
pub const APP_NAME: &str = "git-forest";
#[cfg(feature = "beta")]
pub const APP_NAME: &str = "git-forest-beta";

#[cfg(feature = "stable")]
pub const VERSION_CHANNEL: &str = "stable";
#[cfg(feature = "beta")]
pub const VERSION_CHANNEL: &str = "beta";

pub const VERSION_CHECK_BASE_URL: &str = "https://forest.dliv.gg/api/latest";
pub const INTERNAL_VERSION_CHECK_ARG: &str = "--internal-version-check";
```

Then `config.rs` uses `channel::APP_NAME` instead of its local `const APP_NAME`, `version_check.rs` uses `channel::VERSION_CHECK_BASE_URL` and appends `&channel={VERSION_CHANNEL}`, and both `main.rs` and `version_check.rs` reference `channel::INTERNAL_VERSION_CHECK_ARG`.

### 2. Binary naming: separate binaries, not `conflicts_with`

`git-forest-beta` installs as a distinct binary. Users can have both installed simultaneously.

- Config dirs are automatically separate: `~/.config/git-forest/` vs `~/.config/git-forest-beta/`
- State dirs separate: `~/.local/state/git-forest/` vs `~/.local/state/git-forest-beta/`
- `--internal-version-check` works automatically — `current_exe()` returns whichever binary is running
- Git subcommand discovery works: `git forest-beta new ...`

The `conflicts_with` approach from the original README would force users to uninstall one to install the other, defeating the coexistence goal.

### 3. Worker: query param `?channel=` on same endpoint

The existing endpoint already accepts `?v=` for the caller's version. Add `?channel=stable|beta`:

```
GET /api/latest?v=0.2.17&channel=stable  → {"version": "0.2.17"}
GET /api/latest?v=0.3.0-beta.1&channel=beta  → {"version": "0.3.0-beta.2"}
```

**Chosen over** path-based routing (`/api/beta/latest`) because:
- No new routes needed, same `/api/latest` endpoint
- Backward-compatible — existing clients without `channel` param get stable (default)
- D1 events table adds a `channel` column for analytics

Worker changes:

```toml
# worker/wrangler.toml
[vars]
LATEST_VERSION_STABLE = "0.2.17"
LATEST_VERSION_BETA = "0.3.0-beta.1"
```

```typescript
// worker/src/index.ts
const channel = url.searchParams.get("channel") || "stable";
const latest = channel === "beta"
  ? env.LATEST_VERSION_BETA
  : env.LATEST_VERSION_STABLE;
```

### 4. CI: tag pattern → feature flag

The release workflow already triggers on `tags: ["v*"]`, matching both `v0.2.17` and `v0.3.0-beta.1`. Add a step to parse the tag:

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

Build step becomes:
```yaml
cargo build --release --target ${{ matrix.target }} \
  --no-default-features --features ${{ steps.channel.outputs.features }} \
  --bin ${{ steps.channel.outputs.bin }}
```

The Homebrew update step conditionally updates `git-forest.rb` or `git-forest-beta.rb`. The worker deploy step updates `LATEST_VERSION_STABLE` or `LATEST_VERSION_BETA` (the `just release` / `just release-beta` recipe handles setting the right var in `wrangler.toml` before the CI deploys).

### 5. Homebrew: two formulae, same tap

In `dliv/homebrew-tools`:
- `Formula/git-forest.rb` — stable, same as today
- `Formula/git-forest-beta.rb` — beta builds, `cargo install --no-default-features --features beta --bin git-forest-beta`

No `conflicts_with` — both can be installed. Install beta with `brew install dliv/tools/git-forest-beta`.

### 6. Versioning and tagging

- Stable: `v0.x.y` (e.g., `v0.2.18`)
- Beta: `v0.x.y-beta.N` (e.g., `v0.3.0-beta.1`)
- `Cargo.toml` version matches the tag (semver pre-release is valid: `version = "0.3.0-beta.1"`)
- `just release 0.2.18` — stable release (updates `LATEST_VERSION_STABLE` in wrangler.toml)
- `just release-beta 0.3.0-beta.1` — beta release (updates `LATEST_VERSION_BETA` in wrangler.toml)

### 7. Testing both channels in CI

Add to `ci.yml`:
```yaml
- run: cargo test --no-default-features --features stable
- run: cargo test --no-default-features --features beta
```

The `compile_error!` guard prevents `--all-features`. Features are additive in Cargo, so `--no-default-features` is required when testing beta to avoid enabling both.

## Files Changed (implementation scope)

| File | Changes |
|---|---|
| `Cargo.toml` | Add `[features]`, `[[bin]]` sections |
| `src/channel.rs` | **New.** Channel constants: `APP_NAME`, `VERSION_CHANNEL`, `VERSION_CHECK_BASE_URL`, `INTERNAL_VERSION_CHECK_ARG` |
| `src/main.rs` | Add `mod channel`, use `channel::INTERNAL_VERSION_CHECK_ARG` |
| `src/config.rs` | Replace local `APP_NAME` const with `use crate::channel::APP_NAME` |
| `src/version_check.rs` | Use `channel::*` for URL, channel param, subprocess arg, privacy notice |
| `worker/wrangler.toml` | Replace `LATEST_VERSION` with `LATEST_VERSION_STABLE` + `LATEST_VERSION_BETA` |
| `worker/src/index.ts` | Read `channel` query param, select correct version var |
| `.github/workflows/release.yml` | Add channel detection step, parameterize build/package/homebrew |
| `.github/workflows/ci.yml` | Test both feature sets |
| `justfile` | Add `release-beta` recipe |

## DRY improvements (bundled in)

- `"--internal-version-check"` → `channel::INTERNAL_VERSION_CHECK_ARG` (used in `main.rs` and `version_check.rs`)
- `"https://forest.dliv.gg/api/latest"` → `channel::VERSION_CHECK_BASE_URL` (single source)
- `"git-forest"` in privacy notice → `channel::APP_NAME`

## Open Questions

- **D1 schema migration**: Add `channel TEXT` column to events table? Or infer from version string (beta versions contain `-beta`)? Explicit column is cleaner for queries.
- **`just release-beta` wrangler.toml update**: The `sed` command needs to target `LATEST_VERSION_BETA` specifically. May want to switch from `sed` to a more robust approach (e.g., `toml-cli` or a small script) since there are now two version vars.
- **Worker backward compatibility**: Old clients (pre-channel) won't send `?channel=`. Defaulting to stable is correct. Should the old `LATEST_VERSION` var be kept as an alias during transition?
- **Beta → stable promotion**: When a beta is ready for stable release, is it just a new stable tag at the same commit? Or a separate commit? (Probably just re-tag / new tag.)
