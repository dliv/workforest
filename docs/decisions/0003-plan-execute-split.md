# 3. Plan/Execute Split for Mutations

Date: 2026-02-15
Status: Accepted

## Context

Mutating commands (`new`, `rm`, `init`) touch git and the filesystem. Testing them end-to-end is slow and makes failure diagnosis hard ("which step broke?"). Agents need `--dry-run` to preview actions before approving. Both needs point to separating "decide what to do" from "do it."

## Decision

Mutating commands use a two-phase pattern:

1. **Planning function** — takes inputs and observations and returns a data structure describing intended actions. It writes nothing; `new` fetches each source repository before planning, unless `--no-fetch`, so the plan reflects the remote, and that update of git's remote-tracking refs is the only change a dry-run makes.
2. **Execution function** — carries out the plan (git operations, filesystem writes).

Concrete implementations:

- `plan_forest() -> ForestPlan` / `execute_plan() -> NewResult` (`src/commands/new.rs`). `ForestPlan` contains `Vec<RepoPlan>`, each with a `CheckoutKind` enum (`ExistingLocal`, `TrackRemote`, `NewBranch`; in `crates/forest/core/src/plan/new.rs`) expressing the command pattern as a Rust enum.
- `plan_rm() -> RmPlan` / `execute_rm() -> RmResult` (`src/commands/rm.rs`). `RmPlan` contains `Vec<RepoRmPlan>` (in `crates/forest/core/src/plan/rm.rs`) with pre-checked `worktree_exists` and `source_exists` flags.
- `validate_init_inputs() -> ResolvedConfig` / `write_config_atomic()` (`src/commands/init.rs`) — lighter-weight variant of the same pattern.

`--dry-run` falls out naturally: `cmd_new` calls `plan_forest`, then returns `plan_to_result(&plan, true)` without executing. Same for `cmd_rm`.

## Consequences

- **Testable:** Plan tests assert on data structures without touching git or the filesystem. Execution tests use real repos (ADR 0007) to verify side effects.
- **`--dry-run` for free:** `--json --dry-run` lets agents inspect the full plan before approving.
- **Good error reporting:** Failures reference the specific plan step (e.g., `CreateWorktree` for a specific repo).
- **Command pattern as enums:** `CheckoutKind` and `RmOutcome` make operations explicit and exhaustively matchable.
- Depends on ADR 0002 (plans are data, returned not printed). Enables ADR 0011 (incremental meta writing during execution).
- The decide steps of `new`, `rm` and `reset` and their renderers live in Pure modules under `crates/forest/core/src/plan/`, so the purity and direction scans fail `just check` if planning or rendering reads the filesystem, runs git or imports effectful code; observation and execution stay effectful.

Revisit if: a mutation's later steps depend on what its earlier steps produce, so it cannot be planned up front, or a dry-run is found to predict a different outcome than the run.
