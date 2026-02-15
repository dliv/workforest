# 10. Debug Assertions for Invariants

Date: 2026-02-15
Status: Accepted

## Context

Some conditions indicate bugs in our code (path not absolute after expansion), while others indicate bad user input (malformed config). These need different handling: bug indicators should be loud in development but free in release; user errors should always produce helpful messages. A single error mechanism conflates the two.

## Decision

Use `debug_assert!` for postconditions that should be guaranteed by the code — "the code has a bug" conditions. Use `bail!`/`Result` for conditions caused by user input or external state — "the user gave bad input" conditions.

Current `debug_assert!` usage (all postconditions):

- `src/config.rs` (lines 121–135) — three assertions after `parse_config()`: worktree_base is absolute, repo names non-empty, repo names unique.
- `src/paths.rs` (lines 20–23) — `expand_tilde()` result must not start with `~/`.
- `src/paths.rs` (lines 31–34) — `sanitize_forest_name()` result must not contain `/`.

Corresponding `bail!` usage for user errors:

- `src/config.rs` (lines 97–101) — empty repo name, duplicate repo name.
- `src/config.rs` (lines 74–76) — `feature_branch_template` must contain `{name}`.

**Known gap:** All 7 existing `debug_assert!` calls are postconditions. Zero preconditions exist. The architecture doc calls for preconditions at the start of functions that assume validated input (e.g., `plan_forest()` could assert `repo.path.is_absolute()`). Current assessment: postconditions-only is sufficient because the same test binary exercises both producer and consumer, so postcondition failures in the producer catch the bug before the consumer runs.

## Consequences

- Assertions fire in debug/test builds (`cargo test`), compile away in release.
- Two-tier error model is explicit: `debug_assert!` = internal invariant, `bail!` = external input.
- Code-level complement to ADR 0008 (contract-driven development at the planning level). Together they form a two-tier contract approach: human-readable contracts in plans, machine-checked contracts in assertions.
- Precondition gap is documented; revisit if cross-crate consumers appear.
