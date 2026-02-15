# 12. Forests Are Template-Agnostic After Creation

Date: 2026-02-15
Status: Accepted (not yet implemented — Phase 5B)

## Context

Phase 5B introduces named templates (multiple repo groups in one config). During `new`, the `--template` flag selects which repos to include. The question is whether to store the template name in `.forest-meta.toml` for later reference. Storing it would add a field that no post-creation command uses — informational clutter that creates a false expectation that the template matters after creation.

## Decision

Don't store the template name in `.forest-meta.toml`. The template is a creation-time routing decision, not operational state.

- `ForestMeta` (`src/meta.rs`, lines 23–29) has no `template` field.
- `src/commands/rm.rs`, `status.rs`, `exec.rs` — none reference config templates.
- `main.rs` — post-creation commands load config only for `worktree_base`, then work from meta.
- The template name is available in `NewResult` for the agent that created the forest — it doesn't need to persist.

## Consequences

- **Reinforces ADR 0004:** Meta remains self-contained with no informational-only fields.
- **No false coupling:** Users won't expect that renaming or deleting a template affects existing forests.
- **`ls` is simple:** Shows all forests regardless of which template created them — no filtering/grouping by template needed.
- **Config deletion is safe:** If config is deleted or changed, existing forests still work.
- If template provenance becomes useful later (e.g., "recreate this forest with updated template"), it can be added as an optional field without breaking existing meta files.
