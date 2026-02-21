# Plan: bugs-20260220

Builds on [TRIAGE.md](TRIAGE.md). This plan reflects discussion outcomes that diverge from or refine the triage.

---

## Item 1: Agent instructions hint about stale base refs

**Status:** Ready to implement (accepted as triaged)

Add a one-liner hint to `docs/agent-instructions.md` after the "Review a multi-repo PR" pattern's `# work in ...` comment:

```
# diff against origin/<base>, not local <base> — local may be stale
# git diff origin/main...feature/new-endpoint
```

No code changes, no tests. ~5 minutes.

---

## Item 2: `git forest ls` column alignment — snapshot test

**Status:** Ready to implement (not a bug, but adding regression guard)

### Why not a bug

The formatting code in `ls.rs:96-124` is correct. `ForestName::Display` (`paths.rs:162-165`) writes the raw string via `f.write_str(&self.0)`, and `ForestMode::Display` (`meta.rs:16-22`) writes plain "feature"/"review". Width calculation (`as_str().len()`) matches displayed width exactly.

The misalignment was observed by a Claude Code agent whose rendering collapsed multiple consecutive spaces.

### What to add

A snapshot test using `insta` that captures `format_ls_human` output with varying name lengths, proving alignment is correct and catching any future regression.

### Implementation details

**Add `insta` dev-dependency** to `Cargo.toml`:
```toml
insta = "1"
```

**Add snapshot test** in `ls.rs` tests. The function `format_ls_human` is pure `&LsResult -> String` — no filesystem or git needed. Build an `LsResult` with forests of varying name lengths and snapshot the output:

```rust
#[test]
fn format_ls_human_alignment_snapshot() {
    let result = LsResult {
        forests: vec![
            ForestSummary {
                name: ForestName::new("a".to_string()).unwrap(),
                age_seconds: 300,
                age_display: "5m ago".to_string(),
                mode: ForestMode::Feature,
                branch_summary: vec![BranchCount { branch: "dliv/a".to_string(), count: 2 }],
            },
            ForestSummary {
                name: ForestName::new("review-bar-very-long-name".to_string()).unwrap(),
                age_seconds: 86400,
                age_display: "1d ago".to_string(),
                mode: ForestMode::Review,
                branch_summary: vec![
                    BranchCount { branch: "forest/review-bar".to_string(), count: 2 },
                    BranchCount { branch: "sue/fix-dialog".to_string(), count: 1 },
                ],
            },
            ForestSummary {
                name: ForestName::new("mid-length".to_string()).unwrap(),
                age_seconds: 7200,
                age_display: "2h ago".to_string(),
                mode: ForestMode::Feature,
                branch_summary: vec![BranchCount { branch: "dliv/mid".to_string(), count: 3 }],
            },
        ],
    };
    insta::assert_snapshot!(format_ls_human(&result));
}
```

The `.snap` file will show exact whitespace — every column visibly aligned. Any Display impl change or width calc regression would break the snapshot.

### Notes from deep dive

- `format_ls_human` already has test helpers (`make_meta`, `make_repo`, `ForestName::new`) available in `testutil.rs`, but for snapshots we construct `LsResult`/`ForestSummary` directly since we want to control `age_display` strings exactly (no chrono time dependency).
- `insta` would be the only new dev-dependency. It has no transitive runtime deps. `cargo insta review` provides a TUI for approving snapshots but isn't required — `cargo test` + `INSTA_UPDATE=1` also works.

---

## Item 3: `git forest rm` leaves orphaned state on partial failure

**Status:** Ready to implement

### Root cause

`remove_forest_dir` (rm.rs:301-307) unconditionally deletes `.forest-meta.toml` before checking whether the directory can be removed. When a repo removal fails (dirty files), the meta file is already gone — the forest becomes an orphan: directory exists on disk, but `git forest ls` can't find it and `git forest rm --force` can't target it.

### Design decision: Option A (preflight validation), not Option B

The triage proposed Option B (keep meta file on partial failure, let best-effort accumulation proceed). We chose Option A (preflight check, fail before touching anything) for these reasons:

1. **ADR-0009's "best-effort" is about reporting completeness, not about partial application being desirable.** The intent is "report all 5 repo outcomes, not just the first failure" — information quality, not a mandate to partially mutate.

2. **ADR-0003 (plan/execute split) favors validation in the plan phase.** `plan_rm` already queries filesystem state (`worktree_exists`, `source_exists`). Dirty-state is the same kind of read-only pre-check. Discovering problems during execution means you've already started mutating — which is exactly the bug we're fixing.

3. **Best-effort still applies to genuinely unexpected runtime failures.** A filesystem permission error mid-removal is unpredictable — accumulate and report. Dirty worktrees are predictable and checkable upfront — validate and reject.

4. **Cleaner UX for agents.** An agent gets back either "all clean, rm succeeded" or "these repos are dirty, nothing was touched, re-run with --force." No ambiguous partial state to reason about.

### Implementation approach

**In `RepoRmPlan`:** Add `has_dirty_files: bool` field (rm.rs:16-25).

**In `plan_rm`:** Check dirty state during planning, same pattern as `worktree_exists` / `source_exists` (rm.rs:60-84). Dirty detection: call `crate::git::git(&worktree_path, &["status", "--porcelain"])` — non-empty stdout means dirty. This works correctly in worktrees (`git status` respects the worktree context via `current_dir`). If the worktree doesn't exist, skip the check (`has_dirty_files: false` — nothing to protect).

**In `execute_rm`:** Before the repo loop (rm.rs:96), if `!force`, collect all `repo_plan` where `has_dirty_files` is true. If any found, return early with an `RmResult` where:
- Dirty repos get `RmOutcome::Failed { error: "..." }` for both worktree and branch
- Clean repos get `RmOutcome::Skipped { reason: "blocked by dirty repos" }` (nothing was attempted)
- `forest_dir_removed: false`
- `errors` lists all dirty repos with a hint to use `--force`

**In `remove_forest_dir`:** Fix meta-deletion ordering regardless (rm.rs:300-320). Defense-in-depth for unexpected runtime failures. Move meta file deletion after the `remove_dir` attempt, or gate it on `errors` being empty. Current flow: delete meta (line 302-307) → try `remove_dir` (line 310). New flow: try `remove_dir` first — if it succeeds the meta is gone with the dir; if it fails (not empty), try deleting just the meta and then `remove_dir` again; if *that* fails, the meta stays and the forest remains discoverable.

Actually, simpler: for the non-force path, just attempt `remove_dir` (non-recursive) directly. If the directory only contains `.forest-meta.toml`, `remove_dir` will fail (not empty). So: delete meta, then `remove_dir`. But the key insight is that **we should never reach this code with errors** because the preflight check prevents partial removal. The meta-deletion reorder is defense-in-depth only.

Simplest defense-in-depth: gate the entire `remove_forest_dir` non-force path on `errors.is_empty()`:

```rust
// In execute_rm, after the repo loop:
let forest_dir_removed = if errors.is_empty() {
    remove_forest_dir(&plan.forest_dir, force, &mut errors)
} else {
    // Partial failure (unexpected runtime error) — keep meta for discoverability
    false
};
```

This keeps `remove_forest_dir` unchanged internally but prevents the orphan even for unexpected runtime failures.

**`--dry-run`:** Update `plan_to_dry_run_result` (rm.rs:325-366) to reflect dirty state. If `!force` and any repo has dirty files, the dry-run output should show the rejection (Failed/Skipped) rather than optimistic Success.

### Key findings from deep dive

- **`git` helper** (git.rs:5-30): runs via `Command::new("git").current_dir(repo)`, captures stdout/stderr, bails on non-zero exit. `git status --porcelain` returns exit 0 with empty stdout for clean worktrees — this works cleanly with the helper (returns `Ok("")`). No edge cases to worry about.
- **No existing dirty-check code anywhere** in the codebase — this is new functionality, not a refactor of something existing.
- **Existing test `rm_best_effort_continues_on_failure`** (rm.rs:724-753) makes a worktree dirty by writing+staging a file, then asserts partial removal. This test's assertions will change: with the preflight check, the dirty repo blocks *all* removal, so foo-web should now be `Skipped`, not `Success`.

### Test cases

1. **Dirty repo blocks rm (no --force):** Create forest with 2 repos, dirty one. `rm` without `--force` fails, *nothing* is removed (neither repo, nor meta), forest still appears in `ls`.
2. **--force bypasses dirty check:** Same setup, `rm --force` succeeds, everything cleaned.
3. **Retry after seeing dirty report:** After test 1, clean the dirty repo (or use `--force`), second `rm` succeeds fully.
4. **Clean rm still works:** All repos clean, `rm` removes everything including meta and forest dir. (Covered by existing tests, just verify they still pass.)
5. **Update `rm_best_effort_continues_on_failure`:** Rename to something like `rm_dirty_repo_blocks_all_removal`. Assert: dirty repo → Failed, clean repo → Skipped (not attempted), meta file still exists, forest dir still exists.
6. **Dry-run shows dirty rejection:** `--dry-run` on a forest with dirty repos shows the would-be rejection.

### Effort

~1-2 hours. Small additions to `RepoRmPlan`, dirty check in `plan_rm`, preflight guard in `execute_rm`, defense-in-depth gating on `remove_forest_dir`, `plan_to_dry_run_result` update, test updates.

---

## Implementation order

1. Item 2 (snapshot test — add `insta`, write test, low risk, independent)
2. Item 1 (doc fix — trivial, independent)
3. Item 3 (rm bug — the substantive work)
