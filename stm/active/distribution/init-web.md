Human: this is from Claude web and implementation details will be wrong as it is missing all of the implementation context.

Timeline:

1. ideate with Claude web
2. implement separately with Claude Code / AMP
3. continue with Claude web but missing context from (2)

# forest — Distribution, Homebrew Tap, Version Checking & Telemetry

## Overview

Distribution strategy for `forest` targeting a small audience (primarily macOS users). Priorities: zero-friction install, painless updates, lightweight telemetry via a self-hosted Cloudflare Worker, no heavy infrastructure.

## GitHub Releases

### Build

Use GitHub Actions to cross-compile on release tag:

```yaml
# .github/workflows/release.yml
name: Release

on:
  push:
    tags: ["v*"]

jobs:
  build:
    strategy:
      matrix:
        include:
          - target: aarch64-apple-darwin
            os: macos-latest
            name: forest-aarch64-apple-darwin
          - target: x86_64-apple-darwin
            os: macos-latest
            name: forest-x86_64-apple-darwin
          # Optional: Linux support
          # - target: x86_64-unknown-linux-gnu
          #   os: ubuntu-latest
          #   name: forest-x86_64-unknown-linux-gnu

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
          cp target/${{ matrix.target }}/release/forest dist/${{ matrix.name }}
          cd dist && tar czf ${{ matrix.name }}.tar.gz ${{ matrix.name }}

      - name: Upload
        uses: softprops/action-gh-release@v2
        with:
          files: dist/${{ matrix.name }}.tar.gz

  # Update latest version in Cloudflare KV after release
  update-version:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - name: Update KV with latest version
        env:
          CF_API_TOKEN: ${{ secrets.CF_API_TOKEN }}
          CF_ACCOUNT_ID: ${{ secrets.CF_ACCOUNT_ID }}
          CF_KV_NAMESPACE_ID: ${{ secrets.CF_KV_NAMESPACE_ID }}
        run: |
          VERSION="${GITHUB_REF_NAME#v}"
          curl -X PUT \
            "https://api.cloudflare.com/client/v4/accounts/$CF_ACCOUNT_ID/storage/kv/namespaces/$CF_KV_NAMESPACE_ID/values/latest_version" \
            -H "Authorization: Bearer $CF_API_TOKEN" \
            -H "Content-Type: text/plain" \
            --data "$VERSION"

  # Optional: update Homebrew formula
  homebrew:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - name: Update Homebrew formula
        run: |
          # Calculate SHA256 for each binary, update formula in tap repo
          echo "Update homebrew-tools repo with new version and SHAs"
```

### Release Process

1. Bump version in `Cargo.toml`
2. `git tag v0.2.0 && git push --tags`
3. GitHub Actions builds binaries, uploads to release, updates Cloudflare KV with new version

## Homebrew Tap

### Setup

Create a separate public repo: `github.com/dliv/homebrew-tools`

No registration with Homebrew is required. The naming convention `homebrew-tools` is the entire discovery mechanism — `brew tap dliv/tools` automatically resolves to `github.com/dliv/homebrew-tools`.

Formula file:

```ruby
# Formula/forest.rb
class Forest < Formula
  desc "Multi-repo worktree orchestrator for parallel development"
  homepage "https://github.com/dliv/forest"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/dliv/forest/releases/download/v#{version}/forest-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM"
    elsif Hardware::CPU.intel?
      url "https://github.com/dliv/forest/releases/download/v#{version}/forest-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_INTEL"
    end
  end

  def install
    if Hardware::CPU.arm?
      bin.install "forest-aarch64-apple-darwin" => "forest"
    elsif Hardware::CPU.intel?
      bin.install "forest-x86_64-apple-darwin" => "forest"
    end
  end

  def caveats
    <<~EOS
      To get started, run:
        forest init

      If you use agentic workflows, consider adding forest usage
      instructions to your AGENTS.md or equivalent context file.

      Telemetry: forest checks for updates daily (sends version
      to yourdomain.com). Opt out with:
        forest config set telemetry false
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/forest --version")
  end
end
```

### User Experience

```bash
# One-time setup
brew tap dliv/tools
brew install forest

# Updates
brew upgrade forest
```

### Updating the Formula

On each release, update the version and SHA256 hashes in the formula. Can be automated in the release workflow or done manually:

```bash
curl -sL https://github.com/dliv/forest/releases/download/v0.2.0/forest-aarch64-apple-darwin.tar.gz | shasum -a 256
curl -sL https://github.com/dliv/forest/releases/download/v0.2.0/forest-x86_64-apple-darwin.tar.gz | shasum -a 256
# Update Formula/forest.rb, commit, push to homebrew-tools repo
```

## Telemetry & Version Checking

### Architecture

A single Cloudflare Worker serves as both the version check endpoint and lightweight telemetry collector. It stores events in Cloudflare D1 (SQLite at the edge) and reads the latest version from Cloudflare KV.

```
forest CLI                    Cloudflare Worker               Cloudflare D1 / KV
─────────                    ─────────────────               ──────────────────
GET /api/forest/latest?v=0.2.1  →  log(ip, version, ts) → D1
                                    read latest_version  ← KV
                              ←  { "version": "0.3.0" }
```

One request per day, per user. No additional analytics endpoints.

### Cloudflare Worker

```javascript
// worker.js
export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.pathname !== "/api/forest/latest") {
      return new Response("Not found", { status: 404 });
    }

    const version = url.searchParams.get("v") || "unknown";
    const ip = request.headers.get("cf-connecting-ip") || "unknown";
    const timestamp = new Date().toISOString();

    // Log to D1
    try {
      await env.DB.prepare(
        "INSERT INTO events (ip, version, timestamp) VALUES (?, ?, ?)",
      )
        .bind(ip, version, timestamp)
        .run();
    } catch (e) {
      // Don't fail the response if logging fails
      console.error("D1 write failed:", e);
    }

    // Return latest version from KV
    const latest = await env.KV.get("latest_version");
    return Response.json({
      version: latest || "0.1.0",
    });
  },
};
```

### D1 Schema

```sql
CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ip TEXT NOT NULL,
  version TEXT NOT NULL,
  timestamp TEXT NOT NULL
);

CREATE INDEX idx_events_timestamp ON events(timestamp);
CREATE INDEX idx_events_ip ON events(ip);
```

### Querying Usage

From the Cloudflare dashboard or via `wrangler d1 execute`:

```sql
-- Active users (unique IPs in last 7 days)
SELECT ip, version, COUNT(*) as hits, MAX(timestamp) as last_seen
FROM events
WHERE timestamp > datetime('now', '-7 days')
GROUP BY ip
ORDER BY last_seen DESC;

-- Version adoption
SELECT version, COUNT(DISTINCT ip) as users
FROM events
WHERE timestamp > datetime('now', '-30 days')
GROUP BY version;
```

### Wrangler Config

```toml
# wrangler.toml
name = "forest-api"
main = "worker.js"
compatibility_date = "2024-01-01"

[[d1_databases]]
binding = "DB"
database_name = "forest-telemetry"
database_id = "<your-d1-database-id>"

[[kv_namespaces]]
binding = "KV"
id = "<your-kv-namespace-id>"
```

### Cloudflare Setup Steps

1. `wrangler d1 create forest-telemetry`
2. `wrangler d1 execute forest-telemetry --file=schema.sql`
3. `wrangler kv:namespace create FOREST_KV`
4. `wrangler kv:key put --namespace-id=<id> latest_version "0.1.0"`
5. Update `wrangler.toml` with the IDs
6. `wrangler deploy`
7. Set custom domain route in Cloudflare dashboard (e.g., `yourdomain.com/api/forest/*`)

## Client-Side Implementation

### Compiled-in Version

```rust
const VERSION: &str = env!("CARGO_PKG_VERSION");
```

### Local State

Persist last check time and cached latest version in `~/.config/forest/state.toml`:

```toml
[version_check]
last_checked = "2026-02-15T10:30:00Z"
latest_version = "0.3.0"
```

### Version Check Logic

```rust
use std::time::Duration;
use std::path::Path;

const VERSION_CHECK_URL: &str = "https://yourdomain.com/api/forest/latest";
const CHECK_INTERVAL: Duration = Duration::from_secs(86400); // 24 hours

pub struct UpdateStatus {
    pub latest: String,
    pub update_available: bool,
}

/// Returns Some if there's an update to notify about, None otherwise.
/// Reads from cache if checked recently, hits network at most once per day.
pub fn check_for_update(state_path: &Path) -> Option<UpdateStatus> {
    let current = env!("CARGO_PKG_VERSION");
    let state = read_state(state_path).ok();

    // If checked recently, use cached value
    if let Some(ref s) = state {
        if !is_stale(&s.last_checked, CHECK_INTERVAL) {
            return if version_newer(&s.latest_version, current) {
                Some(UpdateStatus {
                    latest: s.latest_version.clone(),
                    update_available: true,
                })
            } else {
                None
            };
        }
    }

    // Stale or no state — check the network
    let latest = fetch_latest_version(current)?;
    let _ = write_state(state_path, &latest);

    if version_newer(&latest, current) {
        Some(UpdateStatus {
            latest,
            update_available: true,
        })
    } else {
        None
    }
}

fn fetch_latest_version(current: &str) -> Option<String> {
    let url = format!("{}?v={}", VERSION_CHECK_URL, current);
    let response = ureq::get(&url)
        .set("User-Agent", &format!("forest/{}", current))
        .timeout(Duration::from_secs(2))
        .call()
        .ok()?;

    let body: serde_json::Value = response.into_json().ok()?;
    body["version"].as_str().map(|s| s.to_string())
}

fn version_newer(latest: &str, current: &str) -> bool {
    let latest = semver::Version::parse(latest).ok();
    let current = semver::Version::parse(current).ok();
    match (latest, current) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}
```

### Telemetry Opt-Out

Config flag in `~/.config/forest/config.toml`:

```toml
[general]
telemetry = true  # set false to disable version check + telemetry ping
```

On first run (or during `forest init`), display a one-time notice:

```
📡 forest checks for updates daily (sends current version to yourdomain.com).
   Disable with: forest config set telemetry false
```

When `telemetry = false`, skip the network call entirely. The tool works fully offline — no version check, no ping, no update notification.

### User-Facing Output

Print a single line to stderr after successful commands when an update is available:

```
$ forest ls
NAME                          CREATED     BRANCHES
java-84-refactor-auth         2d ago      dliv/java-84/refactor-auth (api, web, infra)

💡 forest v0.3.0 available (current: v0.2.1). Run `forest update` to update.
```

Rules:

- Only print after successful commands
- Print to stderr (don't pollute piped stdout)
- Only check network once per day, use cached result otherwise
- Never block command execution — timeout after 2 seconds, fail silently
- Don't print if already on latest version

### Commands

**`forest version`** — show current version:

```
$ forest version
forest 0.2.1
```

**`forest version --check`** — force a version check:

```
$ forest version --check
forest 0.2.1 (latest: 0.3.0 — run `forest update` to update)
```

**`forest update`** — convenience wrapper:

```rust
pub fn update() -> Result<()> {
    let brew_check = Command::new("brew")
        .args(["--prefix", "forest"])
        .output();

    if brew_check.is_ok() {
        println!("Updating via Homebrew...");
        Command::new("brew")
            .args(["upgrade", "forest"])
            .status()?;
    } else {
        println!("Download the latest release:");
        println!("  https://github.com/dliv/forest/releases/latest");
    }
    Ok(())
}
```

**`forest config set telemetry false`** — opt out of version checks and telemetry.

## Dependencies

| Crate        | Purpose                       | Notes                               |
| ------------ | ----------------------------- | ----------------------------------- |
| `ureq`       | HTTP client for version check | Blocking, minimal, no async runtime |
| `serde_json` | Parse API response            | Likely already a transitive dep     |
| `semver`     | Version comparison            | Tiny                                |

## Implementation Order

1. Add `forest version` command with compiled-in version
2. Set up GitHub Actions release workflow
3. Create Homebrew tap repo with formula
4. Deploy Cloudflare Worker + D1 + KV
5. Add client-side version check logic with state file and caching
6. Add telemetry opt-out config and first-run notice
7. Add `forest update` convenience command
8. Add `update-version` step to release workflow (writes to KV)
