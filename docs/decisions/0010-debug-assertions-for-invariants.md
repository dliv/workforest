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
| `crates/forest/core/src/paths.rs` — `sanitize_forest_name()` | Result has no `/` | Postcondition on a pure helper. Defense-in-depth. |
| `crates/forest/ops/src/config.rs` — `parse_config()` | Repo names unique | Collection-level invariant — no single-value newtype can express set membership. |
| `crates/forest/ops/src/commands/exec.rs` — `cmd_exec()` | `forest_dir.is_absolute()` | Precondition at a `&Path` boundary where the caller passes `&Path`, not `&AbsolutePath`. |
| `crates/forest/ops/src/commands/status.rs` — `cmd_status()` | `forest_dir.is_absolute()` | Same. |
| `crates/forest/core/src/plan/new.rs` — `decide_forest()` | One observation per template repo | Collection-level invariant across two lists: the shell builds the observation from the template's repos, the decision zips the two, and no type ties their lengths. |
| `crates/forest/core/src/plan/init.rs` — `decide_init()` | One observation per `--repo` input | Same kind as `decide_forest()`: the shell observes each `--repo` path in input order, the decision zips the inputs with the observations, and no type ties their lengths. |
| `crates/forest/core/src/plan/rm.rs` — `decide_rm_dry_run()` | One dry-run observation per planned repo, once the root is not refused | Same kind, across the plan and the observation gathered for it; the lengths agree only when the root is not refused, a runtime fact no type carries. |
| `crates/forest/core/src/plan/reset.rs` — `plan_to_dry_run()` | One dry-run observation per forest repo, once the root is not refused | Same as `decide_rm_dry_run()`, for each forest of a reset preview. |

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
- Rust's `debug_assert!` fires by default in debug/test builds — no special flags needed (unlike Java's `-ea`). This makes the dev-time tier practical rather than opt-in. See ADR 0013.

Revisit if: a release build is found to break an invariant that a `debug_assert!` checks only in debug and test builds.
