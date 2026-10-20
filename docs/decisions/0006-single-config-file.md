# 6. Single Config File

Date: 2026-02-15
Status: Accepted (not yet implemented — Phase 5B)

## Context

Phase 5B introduces named templates (multiple repo groups). Templates could live in one file (`~/.config/git-forest/config.toml` with `[template.name]` sections) or in separate files (`~/.config/git-forest/templates/name.toml`). The choice affects parsing, atomic writes, agent ergonomics, and serialization determinism.

A six-criteria evaluation (documented in `PHASE_5_REVIEW_HUMAN_AMP.md`) compared the options. Single-file won on parsing simplicity, atomic write safety, and agent-friendliness. Per-file only wins if users maintain many templates or need import/export — unlikely for v1.

## Decision

All templates live in one `~/.config/git-forest/config.toml`. The file uses a `BTreeMap<String, TemplateConfig>` internally for deterministic serialization order.

Current code already uses a single-file pattern:

- `default_config_path()` (`crates/forest/ops/src/config.rs`) returns one XDG path.
- `write_config_atomic()` (`crates/forest/ops/src/config.rs`) uses temp-file + rename for atomic writes.
- `--force` is only required when overwriting an existing template name, not when adding new ones.

Each section of the file is owned by its reader. The forest's `default_template` and `[template.*]` are parsed by the core's schema (`crates/forest/core/src/config.rs`), which keeps every other top-level key and table and writes them back when `init` rewrites the file, re-serialized as parsed (keys in sorted order, comments and formatting dropped, as the rewrite does for the forest's own keys), so an empty `[version_check]` stays empty and extra keys in it survive. Kept values must be ones TOML can hold: an integer outside TOML's signed 64-bit range in a key or table the forest does not own makes the file fail to load, while an unknown key inside the forest's own tables is skipped unread. The update check's `[version_check]` is read by the update-check edge (`crates/update-check/src/lib.rs`) from the config path in its settings, and a missing or malformed table reads as enabled; the edge skips every other key unread, so nothing elsewhere in the file changes whether the check runs.

## Consequences

- **One parse per reader**, not N-file scanning: the forest parses the file once, and the update check parses it separately for its own table.
- **Atomic writes** via temp-file + rename — no multi-file transactional logic needed.
- **Agent-friendly (ADR 0001):** One file to read, parse, and patch.
- **Deterministic output:** `BTreeMap` gives stable key ordering across serialization cycles.
- **Revisit if:** Users maintain many templates, or template import/export becomes a feature. Per-file would win in those scenarios.
