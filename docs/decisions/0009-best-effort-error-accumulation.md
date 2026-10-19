# 9. Best-Effort Error Accumulation

Date: 2026-02-15
Status: Accepted

## Context

Multi-repo operations can fail partially — one repo's worktree removal might fail while others succeed. A blanket "stop on first error" policy would leave the user with partially cleaned state and no report of what succeeded. A blanket "continue always" policy would be wrong for `new`, where a failed worktree means subsequent repos may depend on broken state. Each command needs its own error policy.

## Decision

Per-command error policies, documented and enforced:

- **`new`:** Stop on first failure and roll back: remove the worktrees this run added, then delete each branch that this run's `git worktree add -b` created, only if its worktree was removed and the branch still points at the commit it was created at (`git update-ref -d` with that commit, a compare-and-delete), then remove the forest directory. A branch another operation created, the branch of an add that failed, and a branch that gained commits during the run all survive. Rollback errors are ignored, and the original error is returned. A failure to write `.forest-meta.toml` returns without rollback, leaving a partial forest whose metadata lists the repos created before it (ADR 0011), so `rm` can clean it up.
- **`rm`:** Best-effort, continue on per-repo failures, report all errors at the end. `RmResult.errors` accumulates `{code, message, hint}` objects (`CleanupIssue` in `crates/forest/core/src/plan/rm.rs`, with a closed `CleanupIssueCode` per cleanup step), and so does `ResetResult.errors`. `RmOutcome` enum has `Success`, `Skipped`, and `Failed` variants.
- **`exec`:** Continue to next repo on failure. `ExecResult.failures` holds one `{repo, code, message, exit_status}` object per failed repo (`ExecRepoFailure` in `crates/forest/core/src/plan/exec.rs`).
- **`status`:** Continue on failure. `RepoStatusKind::Missing` and `Error` variants handle per-repo problems (`crates/forest/core/src/plan/status.rs`).
- **`ls`:** Continue past individual forest directories with missing or unreadable metadata. Return readable forests and structured findings together; failure to enumerate a configured worktree base remains a command error. A dot-prefixed directory without metadata is treated as worktree-base tool or administrative state and omitted from findings; valid or unreadable metadata inside one remains observable.
- **Named forest resolution:** `status <name>`, `exec <name>`, and `rm <name>` continue past unreadable metadata in unrelated directories. Unreadable metadata in a directory matching the requested name remains an error, as do failures to enumerate or inspect the configured base and staged metadata from an interrupted removal.
- **Whole-base mutations:** `rm --all` and `reset` retain strict discovery because incomplete enumeration cannot authorize an all-forests mutation.

Exit codes reflect accumulated state: `main_entry` in `src/lib.rs` exits 1 when a result is incomplete (`rm`, `rm --all` or `reset` errors, `exec` failures) or `reset` needs `--confirm`.

`--force` escalates behavior within a command (e.g., `git branch -d` → `-D` in `rm`) but does not change the accumulation model.

## Consequences

- Errors are data in result structs (ADR 0002), not side effects — `--json` output includes all errors.
- `ls` exits successfully when it produces an inventory result, even when that result contains per-directory findings for callers to evaluate.
- A healthy named forest remains operable when an unrelated forest descriptor is unreadable.
- `rm` of a 5-repo forest reports all 5 outcomes, not just the first failure.
- `RmOutcome` enum makes per-repo results explicit and machine-parseable.
- Partial failure in `new` is recoverable via `rm` because meta is written incrementally (ADR 0011).
- `--force` is an escalation knob, orthogonal to error policy.

Revisit if: a command runs its per-repo steps in parallel, so its first failure no longer has an order, or a caller needs `rm` to stop at its first failure instead of reporting every one.
