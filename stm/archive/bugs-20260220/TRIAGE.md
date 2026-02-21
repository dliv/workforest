# Triage: GIT_FOREST_SUGGESTION_20260220

**Triaged:** 2026-02-20

---

## Item 1: Agent instructions hint about stale base refs

**Type:** Documentation suggestion
**Verdict:** Accept — minimal, helpful, low risk

### Analysis

The `agent-instructions` output is generated from `docs/agent-instructions.md` (included via `include_str!` in `src/main.rs:274`). The "Review a multi-repo PR" pattern at lines 106–113 shows `git forest new` and `git forest rm` but doesn't mention how to diff.

The reporter's scenario is real: an agent following this pattern will diff locally and a stale local base ref produces garbage reviews. Adding a one-liner hint after the `# work in ...` comment is appropriate — git-forest isn't a review tool but the pattern implies a diff will follow.

### Proposed change

In `docs/agent-instructions.md`, after line 111 (`# work in ...`), add:

```
# diff against origin/<base>, not local <base> — local may be stale
# git diff origin/main...feature/new-endpoint
```

### Effort

~5 minutes. One line in a markdown file. No code changes, no tests.

---

## Item 2: `git forest ls` table columns don't align

**Type:** Bug (cosmetic)
**Verdict:** Cannot reproduce — the code looks correct

### Analysis

The formatting logic in `src/commands/ls.rs:96–124` uses `{:<name_width$}` for both the header and data rows, where `name_width` is computed as `max(max_name_len, 4)`. The same `name_width` variable is used in both the header format string (line 111) and the data row format string (line 118):

```rust
// Header
"{:<name_width$}  {:<10}  {:<8}  BRANCHES"
// Data
"{:<name_width$}  {:<10}  {:<8}  {}"
```

This should produce correct alignment. The `ForestName` type implements `Display` (via `#[derive(Serialize)]` and the paths module), so `{:<name_width$}` should pad correctly.

One possible cause: the reporter may have been on v0.2.16 with a different implementation, or the `Display` impl for `ForestName` might include extra formatting. But based on the current code, the alignment logic is correct.

### Suggested next step

Ask the reporter for exact reproduction output, or manually test with forests of varying name lengths. If there's a `Display` impl on `ForestName` that adds quoting or other decoration, the width calculation (which uses `.as_str().len()`) would be off vs the displayed width. Check `ForestName`'s `Display` impl.

### Effort

If a bug exists, likely a one-line fix (use `.as_str()` explicitly in the format, or adjust width calculation). Needs investigation of `ForestName`'s `Display` trait first.

---

## Item 3: `git forest rm` leaves orphaned state on partial failure

**Type:** Bug
**Verdict:** The scenario is real but the reporter's mental model of "internal state tracking" is wrong — the fix is still valid though

### Analysis

git-forest has **no separate state database**. Forest discovery uses `.forest-meta.toml` files in the forest directory itself (`src/forest.rs:20–26`). The `discover_forests` function scans worktree_base for subdirectories containing `.forest-meta.toml`.

The actual failure mode in `remove_forest_dir` (rm.rs:287–321):

1. Without `--force`: the meta file is deleted first (line 303–307), then `remove_dir` (non-recursive) is attempted. If any repo worktree dirs remain (from partial failure), `remove_dir` fails — but **the meta file is already gone**.
2. The forest directory still exists on disk (with orphaned worktree subdirs), but `git forest ls` won't find it because the meta file was deleted.
3. `git forest rm <name> --force` can't find it because the meta file is missing.

So the reporter's scenario is accurate even though the mechanism differs from what they assumed. The meta file deletion at line 303 happens unconditionally before checking if the directory can be cleaned.

### Root cause

`remove_forest_dir` deletes the meta file as a side effect before confirming the directory is actually empty. On partial repo failure, this creates an orphan: directory exists, meta is gone, git-forest can't find it.

### Proposed fix

**Option A (reporter's suggestion — preflight check):** Check all repos for dirty state before removing any. If any are dirty without `--force`, fail early with a list. This changes the ADR-0009 error policy from "best-effort accumulate" to "fail-fast validate" for rm, which is a design change worth discussing.

**Option B (preserve meta on partial failure):** Keep the meta file if any repo removal failed. Only delete it when all repos were successfully cleaned. This preserves the best-effort accumulation model (ADR 0009) — repos that can be cleaned are cleaned, but the forest remains discoverable for retry. The fix is to move the meta-file deletion inside `remove_forest_dir` to be conditional on the directory being empty (or simply skip meta deletion if `errors` is non-empty before calling `remove_forest_dir`).

**Option B is better** — it preserves ADR 0009's intent (rm reports all outcomes, doesn't fail fast) while preventing the orphan. The change is small:

In `execute_rm`, pass the accumulated errors into `remove_forest_dir` and skip meta deletion when there were prior failures:

```rust
let forest_dir_removed = if errors.is_empty() {
    remove_forest_dir(&plan.forest_dir, force, &mut errors)
} else {
    // Don't delete meta — forest needs to stay discoverable for retry
    false
};
```

Or: restructure `remove_forest_dir` to only delete meta when the directory would actually be removable.

### Test cases

1. **Partial failure preserves meta:** Create forest with 2 repos, dirty one, `rm` without `--force`. Assert: clean repo removed, dirty repo failed, `.forest-meta.toml` still exists, `git forest ls` still shows the forest.
2. **Retry with --force after partial failure:** After test 1, `rm --force` succeeds and cleans everything.
3. **Clean rm still removes meta:** All repos clean, `rm` removes everything including meta.

### Effort

~1–2 hours. Small code change in `execute_rm` or `remove_forest_dir`, update the existing `rm_best_effort_continues_on_failure` test, add new test for retry-after-partial-failure.

---

## Priority ranking

1. **Item 3** (medium severity, real data-loss-adjacent bug — orphaned state)
2. **Item 1** (low effort, clear improvement)
3. **Item 2** (needs reproduction first — may not be a current bug)
