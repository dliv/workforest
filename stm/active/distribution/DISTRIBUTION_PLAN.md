# git-forest Distribution Plan

## Goal

Get git-forest installable via Homebrew with automatic update notifications, targeting a small audience (primarily macOS users). Zero-friction install, painless updates, lightweight version checking via a Cloudflare Worker.

## Key Decisions

- **Binary name**: `git-forest`, invoked as `git forest <cmd>` (git subcommand convention)
- **Repo**: `dliv/workforest`
- **Version check endpoint**: `https://forest.dliv.gg/api/latest`
- **Naming**: "version_check" not "telemetry" — be specific about what's happening
- **No configurable API URL** — hardcoded in binary, change in source if needed
- **HTTP client**: `ureq` (blocking, minimal, no async runtime)
- **Config location**: `~/.config/git-forest/config.toml` (Linux) / `~/Library/Application Support/git-forest/config.toml` (macOS), via `directories` crate (already a dependency)
- **State file**: Same directory, `state.toml` — caches last check time and latest known version

## Phases

### Phase 1 — Code Changes + Homebrew Tap ✅

All changes to `dliv/workforest` repo, plus creating `dliv/homebrew-tools`. **Complete.**

1. ✅ `git forest version` / `--version` / `--debug` with compiled-in version
2. ✅ GitHub Actions release workflow (macOS aarch64 + x86_64, triggered on `v*` tags)
3. ✅ Client-side version check module (`ureq` v3, 500ms timeout, daily cache, graceful failure)
4. ✅ Version check opt-out in config (`[version_check]` section)
5. ✅ `git forest update` convenience command (runs `brew update` before `brew upgrade`)
6. ✅ `dliv/homebrew-tools` repo with Homebrew formula

Shipped as v0.2.3. Install: `brew tap dliv/tools && brew install git-forest`

See: [DISTRIBUTION_PHASE_1.md](DISTRIBUTION_PHASE_1.md)

### Phase 2 — Cloudflare Infrastructure (tomorrow)

Standing up the backend that the version check calls.

1. Cloudflare Worker + D1 + KV for version check endpoint
2. DNS: `forest.dliv.gg` subdomain pointing to worker
3. Wire `update-version` step into release workflow (writes new version to KV after build)
4. End-to-end test

See: [DISTRIBUTION_PHASE_2.md](DISTRIBUTION_PHASE_2.md)

## Architecture

```
git forest <cmd>                Cloudflare Worker                Cloudflare D1 / KV
────────────────                ─────────────────                ──────────────────
GET forest.dliv.gg/api/latest     log(ip, version, ts) → D1
    ?v=0.1.0                      read latest_version  ← KV
                                ← { "version": "0.2.0" }

Client rules:
- Check at most once per day (cached in state.toml)
- 500ms timeout, fail silently on any error
- Print update notice to stderr after successful commands
- Skip entirely if version_check.enabled = false in config
```

## Install Experience

```bash
# One-time
brew tap dliv/tools
brew install git-forest

# Updates
brew upgrade git-forest

# Or via built-in command
git forest update
```
