# Config Directory — Switch to XDG Paths

**Goal:** Move config and state from platform-specific directories (`~/Library/Application Support/` on macOS) to XDG-conventional paths (`~/.config/`, `~/.local/state/`).

**Status:** ✅ Complete

**Motivation:** git-forest is a CLI developer tool, not a GUI app. CLI tools conventionally use `~/.config/` on all Unix platforms (gh, git, kubectl, docker, packer, stripe). The current `directories` crate follows Apple's GUI app guidelines, which puts config in `~/Library/Application Support/git-forest/` — hard to discover, annoying to type, and different from Linux.

---

## New Paths

| File | Env override | Default (Unix/macOS) |
|---|---|---|
| config.toml | `$XDG_CONFIG_HOME/git-forest/` | `~/.config/git-forest/` |
| state.toml | `$XDG_STATE_HOME/git-forest/` | `~/.local/state/git-forest/` |

Windows: keep using `directories` crate defaults (no change).

---

## What was done

- Added `xdg_config_dir()` and `xdg_state_dir()` helpers in `src/config.rs` with 3-tier resolution: env var → `$HOME` default → `directories` crate fallback (Windows).
- `default_config_path()` delegates to `xdg_config_dir()`.
- `state_file_path()` in `src/version_check.rs` delegates to `xdg_state_dir()`.
- Updated `tests/cli_test.rs`: removed macOS-specific `~/Library/Application Support/` path expectation, all platforms now expect `~/.config/git-forest/`. Added `env_remove` for XDG vars in test isolation.
- Added 5 unit tests: env var respected (config + state), default paths on Unix (config + state), `default_config_path` ends with `config.toml`.
- ADR written at `docs/decisions/0014-xdg-config-paths.md`.
- All tests pass on macOS and Linux (`just test` + `just test-linux`).

## Checklist

1. [x] Add `xdg_config_dir()` and `xdg_state_dir()` helper functions
2. [x] Update `default_config_path()` to use new paths
3. [x] Update `state_file_path()` to use new paths
4. [x] Add unit tests for path resolution
5. [x] Update integration tests if any assert on path format
6. [x] Run `just check && just test`
7. [x] Commit
