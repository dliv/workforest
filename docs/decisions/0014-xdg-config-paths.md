# 14. XDG Config Paths

Date: 2026-02-18
Status: Accepted

## Context

git-forest uses the `directories` crate (`ProjectDirs::from("", "", "git-forest")`) to determine config and state file locations. On macOS, this resolves to `~/Library/Application Support/git-forest/` — the Apple-recommended location for GUI applications.

However, git-forest is a CLI developer tool, not a GUI app. The developer CLI ecosystem has converged on XDG Base Directory Specification paths (`~/.config/`, `~/.local/state/`, `~/.cache/`) across all Unix platforms, including macOS:

- **git**: `~/.config/git/config`
- **gh** (GitHub CLI): `~/.config/gh/`
- **kubectl**: `~/.config/kube/` (when XDG is set)
- **docker**: `~/.config/docker/` (when XDG is set)
- **packer**: `~/.config/packer/`
- **cargo**: `~/.cargo/` (predates XDG, but home-dir dotfile — same discoverability)

`~/Library/Application Support/` has specific drawbacks for CLI tools: the space in the path requires quoting/escaping, the directory is hidden in Finder by default, and it's a different path from Linux — which breaks muscle memory and dotfile management for developers who work across both platforms.

The Atmos CLI (Go) explicitly migrated from `~/Library/Application Support` to `~/.config` in October 2025, citing "CLI tools should follow CLI conventions, not GUI conventions."

## Decision

Use XDG Base Directory Specification paths on Unix/macOS. Respect `XDG_CONFIG_HOME` and `XDG_STATE_HOME` environment variables. Fall back to `~/.config/git-forest/` and `~/.local/state/git-forest/` when unset. Keep platform-specific paths on Windows via the `directories` crate.

| File | Env override | Default (Unix/macOS) |
|---|---|---|
| `config.toml` | `$XDG_CONFIG_HOME/git-forest/` | `~/.config/git-forest/config.toml` |
| `state.toml` | `$XDG_STATE_HOME/git-forest/` | `~/.local/state/git-forest/state.toml` |

No legacy migration logic. The project is pre-1.0 with no external users.

## Consequences

- **Discoverable.** `~/.config/git-forest/config.toml` is where developers expect CLI config. No more hunting in `~/Library/Application Support/`.
- **Cross-platform consistent.** Same path on Linux and macOS (when `XDG_*` vars aren't overridden). Dotfile repos and setup scripts work without platform conditionals.
- **Respects user overrides.** `XDG_CONFIG_HOME` and `XDG_STATE_HOME` are honored, which matters for developers who customize XDG paths (e.g., NixOS, containerized dev environments).
- **No space in path.** `~/.config/git-forest/` doesn't require quoting in shell commands.
- **ADR 0006 unchanged.** Still a single config file — only the directory location changes.
- **`directories` crate retained.** Still used for Windows defaults.
