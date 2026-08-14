# 9. Best-Effort Error Accumulation

Date: 2026-02-15
Status: Accepted

## Context

Multi-repo operations can fail partially — one repo's worktree removal might fail while others succeed. A blanket "stop on first error" policy would leave the user with partially cleaned state and no report of what succeeded. A blanket "continue always" policy would be wrong for `new`, where a failed worktree means subsequent repos may depend on broken state. Each command needs its own error policy.

## Decision

Per-command error policies, documented and enforced:

- **`new`:** Stop on first failure. The partial forest is left with a valid `.forest-meta.toml` (ADR 0011) so `rm` can clean it up.
- **`rm`:** Best-effort, continue on per-repo failures, report all errors at the end. `RmResult.errors: Vec<String>` accumulates errors (`src/commands/rm.rs`, line 33). `RmOutcome` enum has `Success`, `Skipped`, and `Failed` variants (lines 43–49).
- **`exec`:** Continue to next repo on failure. `ExecResult.failures: Vec<String>` tracks which repos failed (`src/commands/exec.rs`, line 10).
- **`status`:** Continue on failure. `RepoStatusKind::Missing` and `Error` variants handle per-repo problems (`src/commands/status.rs`, lines 21–25).
- **`ls`:** Continue past individual forest directories with missing or unreadable metadata. Return readable forests and structured findings together; failure to enumerate a configured worktree base remains a command error. A dot-prefixed directory without metadata is treated as worktree-base tool or administrative state and omitted from findings; valid or unreadable metadata inside one remains observable.
- **Named forest resolution:** `status <name>`, `exec <name>`, and `rm <name>` continue past unreadable metadata in unrelated directories. Unreadable metadata in a directory matching the requested name remains an error, as do failures to enumerate or inspect the configured base and staged metadata from an interrupted removal.
- **Whole-base mutations:** `rm --all` and `reset` retain strict discovery because incomplete enumeration cannot authorize an all-forests mutation.

Exit codes reflect accumulated state: `main.rs` exits 1 when `rm` has errors (line 149) or `exec` has failures (line 172).

`--force` escalates behavior within a command (e.g., `git branch -d` → `-D` in `rm`) but does not change the accumulation model.

## Consequences

- Errors are data in result structs (ADR 0002), not side effects — `--json` output includes all errors.
- `ls` exits successfully when it produces an inventory result, even when that result contains per-directory findings for callers to evaluate.
- A healthy named forest remains operable when an unrelated forest descriptor is unreadable.
- `rm` of a 5-repo forest reports all 5 outcomes, not just the first failure.
- `RmOutcome` enum makes per-repo results explicit and machine-parseable.
- Partial failure in `new` is recoverable via `rm` because meta is written incrementally (ADR 0011).
- `--force` is an escalation knob, orthogonal to error policy.
