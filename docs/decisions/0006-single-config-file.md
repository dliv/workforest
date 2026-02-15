# 6. Single Config File

Date: 2026-02-15
Status: Accepted (not yet implemented — Phase 5B)

## Context

Phase 5B introduces named templates (multiple repo groups). Templates could live in one file (`~/.config/git-forest/config.toml` with `[template.name]` sections) or in separate files (`~/.config/git-forest/templates/name.toml`). The choice affects parsing, atomic writes, agent ergonomics, and serialization determinism.

A six-criteria evaluation (documented in `PHASE_5_REVIEW_HUMAN_AMP.md`) compared the options. Single-file won on parsing simplicity, atomic write safety, and agent-friendliness. Per-file only wins if users maintain many templates or need import/export — unlikely for v1.

## Decision

All templates live in one `~/.config/git-forest/config.toml`. The file uses a `BTreeMap<String, TemplateConfig>` internally for deterministic serialization order.

Current code already uses a single-file pattern:

- `default_config_path()` (`src/config.rs`, line 46) returns one XDG path.
- `write_config_atomic()` (`src/config.rs`, lines 140–176) uses temp-file + rename for atomic writes.
- `--force` is only required when overwriting an existing template name, not when adding new ones.

## Consequences

- **One `toml::from_str` call** for parsing, not N-file scanning.
- **Atomic writes** via temp-file + rename — no multi-file transactional logic needed.
- **Agent-friendly (ADR 0001):** One file to read, parse, and patch.
- **Deterministic output:** `BTreeMap` gives stable key ordering across serialization cycles.
- **Revisit if:** Users maintain many templates, or template import/export becomes a feature. Per-file would win in those scenarios.
