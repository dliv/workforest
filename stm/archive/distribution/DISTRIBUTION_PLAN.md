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

### Phase 2 — Cloudflare Infrastructure ✅

Standing up the backend that the version check calls. **Complete.**

1. ✅ Cloudflare Worker + D1 + KV for version check endpoint
2. ✅ DNS: `forest.dliv.gg` custom domain via `wrangler.toml` (no dashboard needed)
3. ✅ `update-version` job in release workflow (writes new version to KV after build)
4. ✅ `update-homebrew` job in release workflow (auto-updates formula in `dliv/homebrew-tools`)
5. ✅ Shared types via ts-rs (Rust → TypeScript, one source of truth)
6. ✅ CI: npm audit for worker deps
7. ✅ End-to-end tested through v0.2.6

See: [DISTRIBUTION_PHASE_2.md](DISTRIBUTION_PHASE_2.md) and [CLOUDFLARE_SETUP.md](CLOUDFLARE_SETUP.md)

## Release Pipeline

Pushing a `v*` tag triggers a fully automated pipeline:

1. **build** — Compile macOS aarch64 + x86_64 binaries, upload to GitHub Release
2. **update-version** — Write new version to Cloudflare KV (version check endpoint)
3. **update-homebrew** — Download tarballs, compute SHA256, push updated formula to `dliv/homebrew-tools`

GitHub Secrets: `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`, `CF_KV_NAMESPACE_ID`, `HOMEBREW_TAP_TOKEN`

## Architecture

```
git forest <cmd>                Cloudflare Worker                Cloudflare D1 / KV
────────────────                ─────────────────                ──────────────────
GET forest.dliv.gg/api/latest     log(city, country, version) → D1
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

## Future Improvements

- Add Linux builds to the release matrix
- Retention policy for D1 events (delete old rows periodically)
- Worker deploy via CI (currently manual `just worker-deploy`)
