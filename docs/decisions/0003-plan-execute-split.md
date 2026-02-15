# 3. Plan/Execute Split for Mutations

Date: 2026-02-15
Status: Accepted

## Context

Mutating commands (`new`, `rm`, `init`) touch git and the filesystem. Testing them end-to-end is slow and makes failure diagnosis hard ("which step broke?"). Agents need `--dry-run` to preview actions before approving. Both needs point to separating "decide what to do" from "do it."

## Decision

Mutating commands use a two-phase pattern:

1. **Pure planning function** — takes inputs, returns a data structure describing intended actions. No side effects.
2. **Execution function** — carries out the plan (git operations, filesystem writes).

Concrete implementations:

- `plan_forest() -> ForestPlan` / `execute_plan() -> NewResult` (`src/commands/new.rs`). `ForestPlan` contains `Vec<RepoPlan>`, each with a `CheckoutKind` enum (`ExistingLocal`, `TrackRemote`, `NewBranch` — lines 40–49) expressing the command pattern as a Rust enum.
- `plan_rm() -> RmPlan` / `execute_rm() -> RmResult` (`src/commands/rm.rs`). `RmPlan` contains `Vec<RepoRmPlan>` with pre-checked `worktree_exists` and `source_exists` flags.
- `validate_init_inputs() -> ResolvedConfig` / `write_config_atomic()` (`src/commands/init.rs`) — lighter-weight variant of the same pattern.

`--dry-run` falls out naturally: `cmd_new` calls `plan_forest`, then returns `plan_to_result(&plan, true)` without executing (line 390). Same for `cmd_rm`.

## Consequences

- **Testable:** Plan tests assert on data structures without touching git or the filesystem. Execution tests use real repos (ADR 0007) to verify side effects.
- **`--dry-run` for free:** `--json --dry-run` lets agents inspect the full plan before approving.
- **Good error reporting:** Failures reference the specific plan step (e.g., `CreateWorktree` for a specific repo).
- **Command pattern as enums:** `CheckoutKind` and `RmOutcome` make operations explicit and exhaustively matchable.
- Depends on ADR 0002 (plans are data, returned not printed). Enables ADR 0011 (incremental meta writing during execution).
