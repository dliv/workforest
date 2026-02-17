# Phase 1 — Code Changes + Homebrew Tap

All changes to `dliv/workforest`, plus creating the `dliv/homebrew-tools` repo.

## 1. `git forest version` / `--version` / `--debug`

### What

- Add `#[command(version)]` to the `Cli` struct in `src/cli.rs` — gives `git forest --version` for free via clap
- Add a `Version` subcommand that prints `git-forest <version>` (matches the plan's `git forest version`)
- Add `--check` flag to `Version` that forces a network version check (step 3 dependency)
- Add `--debug` global flag (same level as `--json`)

### Implementation details

**`src/cli.rs`**:

```rust
#[derive(Parser)]
#[command(
    name = "git-forest",
    version = env!("CARGO_PKG_VERSION"),  // NEW — enables --version
    about = "Multi-repo worktree orchestrator",
    after_help = "For AI agent usage instructions: git forest agent-instructions"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true)]  // NEW
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}
```

Add to the `Command` enum:

```rust
/// Show version information
Version {
    /// Check for updates (network call)
    #[arg(long)]
    check: bool,
},
/// Update git-forest to the latest version
Update,
```

**`src/main.rs`**:

```rust
Command::Version { check } => {
    println!("git-forest {}", env!("CARGO_PKG_VERSION"));
    if check {
        // Force version check (ignores daily cache), uses version_check module
    }
}
Command::Update => {
    // Detect brew vs manual, see step 5
}
```

### `--debug` semantics

A simple bool passed through to code that wants to emit diagnostics. No logging framework — just `eprintln!` gated on the flag. Output goes to stderr so it doesn't interfere with `--json` stdout.

Used initially by the version check module (step 3), but available globally for future use. Example output:

```
$ git forest --debug ls
[debug] version check: state file not found, first run
[debug] version check: fetching https://forest.dliv.gg/api/latest?v=0.1.0
[debug] version check: request failed (connection refused), skipping
NAME    CREATED    BRANCHES
...
```

If we later want structured logging across more of the codebase, swap in `tracing` — but for now a bool + `eprintln!` is enough.

### Tests

In `tests/cli_test.rs` (follows existing pattern with `cargo_bin_cmd!("git-forest")`):

- `git forest --version` outputs `git-forest 0.1.0` (success)
- `git forest version` outputs `git-forest 0.1.0` (success)
- `git forest version --check` succeeds and includes version in output (graceful failure when endpoint is unreachable)
- `git forest --debug version` shows debug output on stderr

## 2. GitHub Actions Release Workflow

### What

`.github/workflows/release.yml` triggered on `v*` tags. Cross-compiles macOS aarch64 + x86_64, uploads tarballs to GitHub Releases.

### Workflow

```yaml
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write  # needed for creating releases

jobs:
  build:
    strategy:
      matrix:
        include:
          - target: aarch64-apple-darwin
            os: macos-latest
            name: git-forest-aarch64-apple-darwin
          - target: x86_64-apple-darwin
            os: macos-latest
            name: git-forest-x86_64-apple-darwin

    runs-on: ${{ matrix.os }}

    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Build
        run: cargo build --release --target ${{ matrix.target }}

      - name: Package
        run: |
          mkdir -p dist
          cp target/${{ matrix.target }}/release/git-forest dist/${{ matrix.name }}
          cd dist && tar czf ${{ matrix.name }}.tar.gz ${{ matrix.name }}

      - name: Upload to release
        uses: softprops/action-gh-release@v2
        with:
          files: dist/${{ matrix.name }}.tar.gz
```

### Notes

- Linux builds omitted for now (macOS-only audience)
- The `update-version` job (write to Cloudflare KV) is added in Phase 2
- `permissions: contents: write` is required for `softprops/action-gh-release`

### Release process

1. Bump version in `Cargo.toml`
2. Commit: `chore: bump version to 0.2.0`
3. `git tag v0.2.0 && git push && git push --tags`
4. GitHub Actions builds + uploads

## 3. Client-Side Version Check

### New dependencies

```toml
# Cargo.toml [dependencies]
ureq = { version = "3", features = ["json"] }
semver = "1"
```

`serde_json` is already a dependency. `chrono` with `serde` is already there for timestamps.

Note: ureq 3 is a ground-up rewrite from v2. Key API differences from the original plan's pseudocode:
- Timeouts via `Agent::config_builder().timeout_global()`
- Body access via `.body_mut().read_json::<T>()`
- HTTP 4xx/5xx are errors by default (good for our use case — we treat all failures the same)

### New module: `src/version_check.rs`

This module is entirely imperative shell code (network I/O, file I/O, `eprintln!`). It does not follow the functional-core pattern from ADR-0002 because it's not a command — it's a post-command side effect that runs in `main.rs`.

#### Public API

```rust
pub struct UpdateNotice {
    pub current: String,
    pub latest: String,
}

/// Called after successful commands. Returns Some if an update is available.
/// All errors are swallowed — returns None on any failure.
pub fn check_for_update(debug: bool) -> Option<UpdateNotice> { ... }

/// Called by `git forest version --check`. Forces a network check (ignores cache).
/// Returns None on network failure.
pub fn force_check(debug: bool) -> Option<UpdateNotice> { ... }

/// Returns true if version checking is enabled in config.
/// Returns true if config doesn't exist or can't be read (default-on).
pub fn is_enabled() -> bool { ... }
```

#### Network request (ureq v3 API)

```rust
use std::time::Duration;
use ureq::Agent;
use serde::Deserialize;

const VERSION_CHECK_URL: &str = "https://forest.dliv.gg/api/latest";

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

fn fetch_latest_version(current: &str, debug: bool) -> Option<String> {
    let url = format!("{}?v={}", VERSION_CHECK_URL, current);

    if debug {
        eprintln!("[debug] version check: fetching {}", url);
    }

    let config = Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(500)))
        .build();
    let agent: Agent = config.into();

    let resp: VersionResponse = agent.get(&url)
        .header("User-Agent", &format!("git-forest/{}", current))
        .call()
        .ok()?
        .body_mut()
        .read_json::<VersionResponse>()
        .ok()?;

    Some(resp.version)
}
```

#### Version comparison

```rust
fn version_newer(latest: &str, current: &str) -> bool {
    let latest = semver::Version::parse(latest).ok();
    let current = semver::Version::parse(current).ok();
    match (latest, current) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}
```

### State file

**Location** — derived from `directories::ProjectDirs`, using `state_dir()` with fallback to `data_local_dir()` (because `state_dir()` returns `None` on macOS):

```rust
fn state_file_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "git-forest")?;
    let dir = proj.state_dir().unwrap_or_else(|| proj.data_local_dir());
    Some(dir.join("state.toml"))
}
```

Resulting paths:
| Platform | Path |
|----------|------|
| **macOS** | `~/Library/Application Support/git-forest/state.toml` |
| **Linux** | `~/.local/state/git-forest/state.toml` |

Note: on macOS this is the same directory as `config.toml`. On Linux they're separate (config in `~/.config/`, state in `~/.local/state/`), which follows XDG conventions.

**Format:**

```toml
[version_check]
last_checked = "2026-02-16T10:30:00Z"
latest_version = "0.2.0"
```

Read/write via serde + toml, same pattern as `ForestMeta::read()`/`ForestMeta::write()` in `src/meta.rs`.

**Why cache `latest_version`?** We only hit the network once per day, but the user may run many commands in that window. Caching the last API response lets every command compare the compiled-in version against the cached latest and show the update notice — similar to a persistent "update available" badge in a web app. Without the cache, only the single command that triggered the network call would see the notice.

### `check_for_update()` flow

```
1. is_enabled()? → if false, return None
2. Read state.toml
   - File missing? → first run (show first-run notice, proceed to network check)
   - File exists, last_checked < 24h ago? → compare cached latest_version vs current, return
   - File exists, last_checked >= 24h ago? → proceed to network check
3. fetch_latest_version(current)
   - Success? → write state.toml, compare, return
   - Failure? → return None (fail silent)
```

### Integration in `main.rs`

After `run(cli)` succeeds and returns `Ok(())`, call `version_check::check_for_update(cli.debug)`. If it returns `Some(notice)`, print to stderr:

```
Update available: git-forest v0.3.0 (current: v0.1.0). Run `git forest update` to upgrade.
```

Which commands trigger the post-command version check:
| Command | Triggers check? | Reason |
|---------|----------------|--------|
| `init`, `new`, `rm`, `ls`, `status`, `exec` | Yes | Normal commands |
| `version` (no flag) | No | Just prints version |
| `version --check` | No | Handles its own check via `force_check()` |
| `update` | No | Already updating |
| `agent-instructions` | No | Output is consumed by agents, not humans |

Implementation: match on command variant before calling `check_for_update()`.

Rules:
- Only after successful commands (not on error exit)
- Print to stderr (don't pollute piped stdout)
- Never block — 500ms timeout on network, fail silently
- Skip if `version_check.enabled = false` in config
- Shows on every command while an update is available (not just the command that triggered the network check)

### Config integration

Add an optional `version_check` field to the existing `MultiTemplateConfig` struct in `src/config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTemplateConfig {
    pub default_template: String,
    pub template: BTreeMap<String, TemplateConfig>,
    #[serde(default)]  // existing configs without this section still parse
    pub version_check: Option<VersionCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCheckConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }
```

Behavior:
- `[version_check]` section absent → version check enabled (default)
- `[version_check]` present, `enabled` absent → enabled (default true)
- `[version_check] enabled = false` → disabled

Important: `version_check` config is only read, never written by git-forest. Users edit it manually. The `write_config_atomic()` function in `config.rs` currently serializes `MultiTemplateConfig` — we need to make sure it preserves the `version_check` section if present. Since it round-trips through the same struct, this should work automatically.

Edge case: commands that don't require config (like `version`, `init --show-path`) — `is_enabled()` should handle the case where config doesn't exist by defaulting to enabled.

### Tests

Unit tests in `src/version_check.rs`:
- `version_newer("0.2.0", "0.1.0")` → true
- `version_newer("0.1.0", "0.1.0")` → false
- `version_newer("0.1.0", "0.2.0")` → false
- `version_newer("invalid", "0.1.0")` → false
- `version_newer("0.2.0", "invalid")` → false
- State file read/write round-trip
- Staleness check: >24h returns true, <24h returns false

Integration tests in `tests/cli_test.rs`:
- `git forest version --check` doesn't crash when endpoint is unreachable (graceful failure)
- `git forest --debug version --check` shows debug output on stderr

Config backwards compatibility test in `src/config.rs`:
- Existing config TOML without `[version_check]` still parses successfully

## 4. Version Check Opt-Out

### First-run notice

On the first command that triggers a version check (i.e., state file doesn't exist yet), print to stderr before the check:

```
Note: git-forest checks for updates daily (current version sent to forest.dliv.gg).
Disable: set version_check.enabled = false in config.
```

Only printed once (state file existence serves as the "already shown" flag).

### Behavior when disabled

When `version_check.enabled = false`:
- Skip network call entirely
- Don't read/write state file
- `git forest version --check` prints a message saying version check is disabled

## 5. `git forest update` Command

### What

Convenience wrapper that detects install method and runs the appropriate update.

### Implementation

Add `Update` variant to `Command` enum in `cli.rs` (shown in step 1 above).

Handle in `main.rs` — this command prints directly rather than returning a result struct, since it delegates to an external process (`brew`). This is similar to how `AgentInstructions` uses `print!` directly.

Logic:
1. Check if installed via Homebrew: `brew --prefix git-forest` succeeds
2. If yes: run `brew upgrade git-forest` (inherits stdin/stdout/stderr for interactive output)
3. If no: print link to GitHub releases page

```rust
Command::Update => {
    let brew_check = std::process::Command::new("brew")
        .args(["--prefix", "git-forest"])
        .output();

    if brew_check.map(|o| o.status.success()).unwrap_or(false) {
        println!("Updating via Homebrew...");
        let status = std::process::Command::new("brew")
            .args(["upgrade", "git-forest"])
            .status()?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
    } else {
        println!("Download the latest release:");
        println!("  https://github.com/dliv/workforest/releases/latest");
    }
}
```

```
$ git forest update
Updating via Homebrew...
# (brew upgrade output)

$ git forest update  # (when not installed via brew)
Download the latest release:
  https://github.com/dliv/workforest/releases/latest
```

### Tests

Integration test: `git forest update` doesn't crash (prints either brew output or download link depending on environment).

## 6. Homebrew Tap

### Setup

Create public repo: `github.com/dliv/homebrew-tools`

### Formula: `Formula/git-forest.rb`

```ruby
class GitForest < Formula
  desc "Multi-repo worktree orchestrator for parallel development"
  homepage "https://github.com/dliv/workforest"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/dliv/workforest/releases/download/v#{version}/git-forest-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM"
    elsif Hardware::CPU.intel?
      url "https://github.com/dliv/workforest/releases/download/v#{version}/git-forest-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_INTEL"
    end
  end

  def install
    if Hardware::CPU.arm?
      bin.install "git-forest-aarch64-apple-darwin" => "git-forest"
    elsif Hardware::CPU.intel?
      bin.install "git-forest-x86_64-apple-darwin" => "git-forest"
    end
  end

  def caveats
    <<~EOS
      git-forest is invoked as a git subcommand:
        git forest init --help

      For agentic workflows, add this to your project's AGENTS.md or CLAUDE.md:

        ## git-forest

        This project uses `git forest` to manage multi-repo worktrees for
        feature development and PR review. When the user asks to create a
        forest, worktree environment, or review a PR across repos,
        run `git forest agent-instructions` for full usage guidance.

      Version checking: git-forest checks for updates daily
      (sends current version to forest.dliv.gg). Disable in config:
        Set version_check.enabled = false in ~/.config/git-forest/config.toml
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/git-forest --version")
  end
end
```

### Install experience

```bash
brew tap dliv/tools
brew install git-forest

# Invoked as:
git forest ls
```

### Tap repo structure

```
dliv/homebrew-tools/
  Formula/
    git-forest.rb
  README.md  (optional)
```

### Formula updates

After each release, update version + SHA256 hashes in the formula. This can be automated in Phase 2 (release workflow pushes to the tap repo) or done manually:

```bash
# Download tarballs, compute SHA
curl -sL https://github.com/dliv/workforest/releases/download/v0.2.0/git-forest-aarch64-apple-darwin.tar.gz | shasum -a 256
curl -sL https://github.com/dliv/workforest/releases/download/v0.2.0/git-forest-x86_64-apple-darwin.tar.gz | shasum -a 256
# Update Formula/git-forest.rb, commit, push
```
