# 10. Debug Assertions for Invariants

Date: 2026-02-15
Status: Accepted (updated after Contracts 1 & 2)

## Context

Some conditions indicate bugs in our code (path not absolute after expansion), while others indicate bad user input (malformed config). These need different handling: bug indicators should be loud in development but free in release; user errors should always produce helpful messages. A single error mechanism conflates the two.

## Decision

Use `debug_assert!` for postconditions that should be guaranteed by the code — "the code has a bug" conditions. Use `bail!`/`Result` for conditions caused by user input or external state — "the user gave bad input" conditions.

**Prefer newtypes over assertions.** Where an invariant can be encoded as a type (`AbsolutePath`, `RepoName`, `ForestName`, `BranchName`), the type replaces both the `debug_assert!` postcondition and the `bail!` validation — the constructor validates once, and the type system guarantees the invariant everywhere else. Assertions remain only for invariants that types cannot express (collection-level properties, preconditions at `&Path` boundaries).

### Current `debug_assert!` usage

After Contracts 1 (AbsolutePath) and Contracts 2 (RepoName, ForestName, BranchName):

| Location | Assertion | Why kept |
|----------|-----------|----------|
| `src/paths.rs` — `sanitize_forest_name()` | Result has no `/` | Postcondition on a pure helper. Defense-in-depth. |
| `src/config.rs` — `parse_config()` | Repo names unique | Collection-level invariant — no single-value newtype can express set membership. |
| `src/commands/exec.rs` — `cmd_exec()` | `forest_dir.is_absolute()` | Precondition at a `&Path` boundary where the caller passes `&Path`, not `&AbsolutePath`. |
| `src/commands/status.rs` — `cmd_status()` | `forest_dir.is_absolute()` | Same. |

### Assertions eliminated by newtypes

| Count | What | Replaced by |
|-------|------|-------------|
| 3 | Path-is-absolute postconditions | `AbsolutePath` newtype (Contracts 1) |
| 2 | Repo-name-non-empty postconditions | `RepoName` newtype (Contracts 2) |
| 1 | `~/` prefix postcondition in `expand_tilde` | `AbsolutePath` return type (Contracts 1) |
| 1 | `validate_branch_name()` helper | `BranchName::new()` constructor (Contracts 2) |

## Consequences

- Assertions fire in debug/test builds (`cargo test`), compile away in release.
- Three-tier error model: newtypes (compile-time) > `debug_assert!` (dev-time) > `bail!` (runtime, user-facing). See ADR 0008.
- Net: 7 original assertions reduced to 4 (2 postconditions kept, 2 preconditions added). The type system now carries most of the weight.
