# 8. Contract-Driven Development

Date: 2026-02-15
Status: Accepted

## Context

git-forest is built by a human architect specifying designs and an AI agent implementing them. Without explicit contracts, the agent may drift from the design — inventing types, omitting edge cases, or diverging on naming. The development process needs a mechanism to keep specification and implementation aligned.

## Decision

Each phase is specified as a plan document (`PHASE_*_PLAN.md`) that defines types, interfaces, and test cases before implementation begins. Plans serve as contracts between architect and agent.

Plans specify:
- **Types and structs** — `ForestPlan`, `RepoPlan`, `CheckoutKind` in `PHASE_3_PLAN.md` map directly to `src/commands/new.rs` (lines 22–49). `RmPlan`, `RmOutcome` in `PHASE_4_PLAN.md` map to `src/commands/rm.rs` (lines 9–49).
- **Test names and behaviors** — test names in code match plan specs (e.g., `plan_empty_name_errors`, `rm_removes_worktrees`). Test coverage is designed, not discovered.
- **Review before coding** — review docs (`PHASE_3_PLAN_REVIEW_AMP.md`, `PHASE_5_REVIEW_AMP.md`) catch issues before implementation starts.

The development cycle is: plan → review → implement → archive. Plans become historical after implementation; live decisions migrate to ADRs (this document set).

## Consequences

- Implementation fills in a pre-defined shape — types exist in the plan before they exist in code.
- Review docs prevent rework by catching design issues early.
- Plans are disposable after implementation; ADRs are the durable record.
- Three-tier contract approach:
  1. **Human-level contracts** (this ADR) — plans define types, interfaces, and test cases before implementation.
  2. **Compile-time contracts** — newtypes (`AbsolutePath`, `RepoName`, `ForestName`, `BranchName`) make illegal states unrepresentable. Validate once at construction; the type system enforces invariants everywhere else.
  3. **Runtime contracts** (ADR 0010) — `debug_assert!` for invariants that types cannot express (collection-level properties, `&Path` boundary preconditions).
