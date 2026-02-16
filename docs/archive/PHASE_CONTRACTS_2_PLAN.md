# Phase Contracts 2 — String Newtypes & Remaining Assertions

**Branch:** `dliv/setup`
**Predecessor:** Phase Contracts 1 (`AbsolutePath` newtype)
**Baseline:** Whatever Contracts 1 leaves (164+ tests).

Goal: Introduce `RepoName`, `ForestName`, and `BranchName` newtypes for String fields that carry implicit invariants. Add remaining `debug_assert!` preconditions/postconditions that survive the newtype treatment. Update ADR 0010 to reflect the final state.

---

## 1. The Newtypes

### 1a. `RepoName` (in `src/paths.rs`)

**Invariant:** Non-empty string.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoName(String);

impl RepoName {
    pub fn new(name: String) -> Result<Self> {
        if name.is_empty() {
            bail!("repo name must not be empty");
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RepoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
```

Plus `Serialize`/`Deserialize` (validate on deserialize, same pattern as `AbsolutePath`).

**Fields affected:**

| Struct | Field | File |
|--------|-------|------|
| `ResolvedRepo` | `name` | `config.rs` |
| `RepoMeta` | `name` | `meta.rs` |
| `RepoPlan` | `name` | `new.rs` |
| `RepoRmPlan` | `name` | `rm.rs` |
| `NewRepoResult` | `name` | `new.rs` |
| `RepoRmResult` | `name` | `rm.rs` |
| `RepoStatus` | `name` | `status.rs` |
| `ExecResult.failures` | elements | `exec.rs` (this is `Vec<String>` — keep as `String` since failures are a subset, not a newtype boundary) |

**Assertions eliminated:**
- `config.rs` — `debug_assert!(resolved_tmpl.repos.iter().all(|r| !r.name.is_empty()))` — **remove**
- `init.rs` — `debug_assert!(resolved_repos.iter().all(|r| !r.name.is_empty()))` — **remove**
- `config.rs` — `bail!("repo has empty name")` in `parse_config()` — **replaced by `RepoName::new()` constructor** at the same call site. The `bail!` moves into the constructor. The error message stays user-facing (it's `Result`, not `debug_assert!`).
- `init.rs` — `bail!("repo has empty name")` in `validate_init_inputs()` — same treatment.

**Assertions kept:**
- `config.rs` — `debug_assert!(names.len() == repos.len())` (uniqueness) — uniqueness is a collection-level property, not a single-value invariant. A newtype can't express it. Keep the assertion.

### 1b. `ForestName` (in `src/paths.rs`)

**Invariant:** Non-empty, not `.` or `..`.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForestName(String);

impl ForestName {
    pub fn new(name: String) -> Result<Self> {
        if name.is_empty() || name == "." || name == ".." {
            bail!(
                "invalid forest name: {:?}\n  hint: provide a descriptive name like \"java-84/refactor-auth\"",
                name
            );
        }
        let sanitized = super::sanitize_forest_name(&name);
        if sanitized.is_empty() {
            bail!(
                "forest name {:?} sanitizes to empty\n  hint: provide a name with at least one alphanumeric character",
                name
            );
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The filesystem-safe form (slashes replaced with hyphens).
    pub fn sanitized(&self) -> String {
        super::sanitize_forest_name(&self.0)
    }
}

impl std::fmt::Display for ForestName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
```

Plus `Serialize`/`Deserialize`.

**Fields affected:**

| Struct | Field | File |
|--------|-------|------|
| `NewInputs` | `name` | `new.rs` |
| `ForestPlan` | `forest_name` | `new.rs` |
| `NewResult` | `forest_name` | `new.rs` |
| `ForestMeta` | `name` | `meta.rs` |
| `RmPlan` | `forest_name` | `rm.rs` |
| `RmResult` | `forest_name` | `rm.rs` |
| `ExecResult` | `forest_name` | `exec.rs` |
| `StatusResult` | `forest_name` | `status.rs` |
| `ForestSummary` | `name` | `ls.rs` |

**Code eliminated from `plan_forest()`:**
```rust
// These 10 lines move into ForestName::new()
if inputs.name.is_empty() || inputs.name == "." || inputs.name == ".." {
    bail!(...);
}
let sanitized = sanitize_forest_name(&inputs.name);
if sanitized.is_empty() {
    bail!(...);
}
```

**`forest_dir` changes:**
```rust
// BEFORE (after Contracts 1)
pub fn forest_dir(worktree_base: &AbsolutePath, name: &str) -> AbsolutePath

// AFTER
pub fn forest_dir(worktree_base: &AbsolutePath, name: &ForestName) -> AbsolutePath
```

Uses `name.sanitized()` internally.

**`sanitize_forest_name` visibility:** Becomes `pub(crate)` or private — only called by `ForestName::sanitized()` and `forest_dir()`. The `debug_assert!` inside it is **kept** (it's a postcondition on the helper, not redundant with the newtype — `ForestName` validates the input, `sanitize_forest_name` validates its own output).

### 1c. `BranchName` (in `src/paths.rs`)

**Invariant:** Non-empty, doesn't start with `refs/`, doesn't start with `<remote>/` (remote-aware validation).

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    /// Validate a branch name.
    /// `remote` is needed to reject remote-prefixed names like "origin/main".
    pub fn new(name: String, remote: &str) -> Result<Self> {
        if name.is_empty() {
            bail!("branch name must not be empty");
        }
        if name.starts_with("refs/") {
            bail!(
                "branch name {:?} looks like a ref path\n  hint: pass the branch name without the refs/ prefix",
                name
            );
        }
        let remote_prefix = format!("{}/", remote);
        if name.starts_with(&remote_prefix) {
            bail!(
                "branch name {:?} looks like a remote ref\n  hint: pass the branch name without the remote prefix: {:?}",
                name,
                &name[remote_prefix.len()..]
            );
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
```

Plus `Serialize` (for JSON output). **No `Deserialize`** — branch names in meta files don't need re-validation (they were validated at creation time, and the remote context isn't available at deserialization). Keep `RepoMeta.branch` and `RepoMeta.base_branch` as `String`.

**Decision: Narrower scope than other newtypes.** `BranchName` is only used in the `plan_forest()` pipeline — from `compute_target_branch()` output through `RepoPlan.branch`. It is NOT used in `ForestMeta` (persisted state) or in command results (output structs). This avoids the problem of needing a remote for deserialization.

**Fields affected:**

| Struct | Field | File |
|--------|-------|------|
| `RepoPlan` | `branch` | `new.rs` |
| `RepoPlan` | `base_branch` | `new.rs` — **No**, base_branch doesn't go through the same validation. Keep as `String`. |

**Code eliminated from `plan_forest()`:**
- `validate_branch_name()` function — **replaced by `BranchName::new()` constructor**
- 3 call sites of `validate_branch_name()` in `plan_forest()` — become `BranchName::new(branch, &repo.remote)?`

**Code in `compute_target_branch()`:**
```rust
// BEFORE
fn compute_target_branch(...) -> String

// AFTER — still returns String
// BranchName::new() is called by the caller after computing the raw string
```

This keeps `compute_target_branch` pure (no error path, no remote dependency). The caller wraps the result in `BranchName::new()`.

---

## 2. Remaining `debug_assert!` After Both Phases

After Contracts 1 + 2, the complete set of assertions in the codebase:

### Kept Postconditions

| Location | Assertion | Why Kept |
|----------|-----------|----------|
| `paths.rs` | `sanitize_forest_name` result has no `/` | Postcondition on a helper. Could be removed if `sanitize` becomes a private impl detail of `ForestName`, but the function is small and the assertion is cheap. **Keep.** |
| `config.rs` | Repo names unique | Collection-level invariant — no single-value newtype can express it. **Keep.** |

### New Preconditions (added in this phase)

| Location | Assertion | Rationale |
|----------|-----------|-----------|
| `cmd_exec()` | `debug_assert!(forest_dir.is_absolute())` | Added in Contracts 1. Takes `&Path`, not `&AbsolutePath`. |
| `cmd_status()` | `debug_assert!(forest_dir.is_absolute())` | Same. |

### Eliminated (total across both phases)

| Count | What | By |
|-------|------|----|
| 3 | Path-is-absolute postconditions | `AbsolutePath` newtype (Contracts 1) |
| 2 | Repo-name-non-empty postconditions | `RepoName` newtype (Contracts 2) |
| 1 | `~/` prefix postcondition in `expand_tilde` | `AbsolutePath` return type (Contracts 1) |

Net: 7 assertions removed (the original ADR 0010 count minus the 2 that survived), 2 preconditions added, for a final count of 4.

---

## 3. File-by-File Changes

### `src/paths.rs` — Major Changes

| Item | Change |
|------|--------|
| `RepoName` struct | Add with `new()`, `as_str()`, `Display`, `Serialize`, `Deserialize` |
| `ForestName` struct | Add with `new()`, `as_str()`, `sanitized()`, `Display`, `Serialize`, `Deserialize` |
| `BranchName` struct | Add with `new()`, `as_str()`, `Display`, `Serialize` (no `Deserialize`) |
| `forest_dir()` | Takes `&ForestName` instead of `&str`, uses `name.sanitized()` |
| `sanitize_forest_name()` | Make `pub(crate)` — only used by `ForestName::sanitized()` and `forest_dir()` |
| Tests | Add constructor tests for each newtype. Update `forest_dir` tests. |

### `src/config.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `ResolvedRepo.name` | `String` → `RepoName` |
| `parse_config()` | Replace `bail!("empty name")` with `RepoName::new(name)?`. Remove `debug_assert!` for non-empty names. |
| `debug_assert!` (uniqueness) | **Keep** — add comment: "collection-level invariant, not expressible as newtype" |
| `write_config_atomic()` | `RepoName` serializes as `String` via `Serialize` impl — no change needed |
| `HashSet<String>` for names | Becomes `HashSet<RepoName>` or keep as `HashSet<String>` using `name.as_str()`. **Decision: keep as `HashSet<String>`** — the set is temporary, used only for duplicate detection. No need to change. |
| Tests | Mechanical — wrap repo names in `RepoName::new()` where `ResolvedRepo` is constructed directly |

### `src/meta.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `ForestMeta.name` | `String` → `ForestName` |
| `RepoMeta.name` | `String` → `RepoName` |
| `RepoMeta.branch` | Keep as `String` — branch names in meta aren't validated at read time |
| `RepoMeta.base_branch` | Keep as `String` |
| Tests | Update `sample_meta()` and test helpers to use `ForestName::new()` / `RepoName::new()` |

### `src/commands/new.rs` — Significant Changes

| Item | Change |
|------|--------|
| `NewInputs.name` | `String` → keep as `String`. Construct `ForestName` at the start of `plan_forest()`. The CLI layer passes raw strings; validation happens at the command boundary. |
| `ForestPlan.forest_name` | `String` → `ForestName` |
| `RepoPlan.name` | `String` → `RepoName` |
| `RepoPlan.branch` | `String` → `BranchName` |
| `NewResult.forest_name` | `String` → `ForestName` |
| `NewRepoResult.name` | `String` → `RepoName` |
| `NewRepoResult.branch` | Keep as `String` — output struct for JSON, no need for newtype |
| `plan_forest()` | Replace validation block (lines 115–127) with `let forest_name = ForestName::new(inputs.name.clone())?`. Replace `validate_branch_name()` calls with `BranchName::new()`. |
| `validate_branch_name()` | **Remove** — replaced by `BranchName::new()` |
| `compute_target_branch()` | Keep returning `String` — the `BranchName` wrapping happens at the call site |
| `execute_plan()` | `RepoPlan.branch` is `BranchName` — use `.as_str()` when passing to git commands |
| Tests | Moderate — update plan assertions to use `.as_str()` for comparisons |

### `src/commands/rm.rs` — Minor Changes

| Item | Change |
|------|--------|
| `RmPlan.forest_name` | `String` → `ForestName` |
| `RepoRmPlan.name` | `String` → `RepoName` |
| `RmResult.forest_name` | `String` → `ForestName` |
| `RepoRmResult.name` | `String` → `RepoName` |
| `plan_rm()` | `meta.name` is `ForestName`, flows through. `repo.name` is `RepoName`, flows through. |
| Tests | Mechanical |

### `src/commands/exec.rs` — Minor Changes

| Item | Change |
|------|--------|
| `ExecResult.forest_name` | `String` → `ForestName` |
| `ExecResult.failures` | Keep as `Vec<String>` — failure names are derived from `RepoName.as_str()` |

### `src/commands/status.rs` — Minor Changes

| Item | Change |
|------|--------|
| `StatusResult.forest_name` | `String` → `ForestName` |
| `RepoStatus.name` | `String` → `RepoName` |

### `src/commands/ls.rs` — Minor Changes

| Item | Change |
|------|--------|
| `ForestSummary.name` | `String` → `ForestName` |

### `src/commands/init.rs` — Minor Changes

| Item | Change |
|------|--------|
| `validate_init_inputs()` | Replace `bail!("repo has empty name")` with `RepoName::new(name)?`. Remove `debug_assert!` for non-empty names. |

### `src/testutil.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `make_meta()` | `name` param becomes `&str`, constructs `ForestName::new(name.to_string()).unwrap()` |
| `make_repo()` | `name` param becomes `&str`, constructs `RepoName::new(name.to_string()).unwrap()` |

### `src/forest.rs` — Minor Changes

| Item | Change |
|------|--------|
| `find_forest()` | `name_or_dir` stays `&str` — this is user input, not yet validated. Comparison uses `meta.name.as_str() == name_or_dir`. |
| `discover_forests()` | No change — returns `Vec<ForestMeta>` which now has `ForestName` fields, but that's transparent. |

### `src/main.rs` — No Changes Expected

CLI strings pass into command functions as `String`. Validation happens inside the command layer.

---

## 4. Execution Order

1. **`src/paths.rs`** — Add `RepoName`, `ForestName`, `BranchName` structs. Update `forest_dir()` to take `&ForestName`. Update tests.
2. **`src/config.rs`** — Change `ResolvedRepo.name` to `RepoName`. Update `parse_config()`. Remove name assertions. Update tests.
3. **`src/meta.rs`** — Change `ForestMeta.name` to `ForestName`, `RepoMeta.name` to `RepoName`. Update tests.
4. **`src/testutil.rs`** — Update helpers.
5. **`src/commands/init.rs`** — Update `validate_init_inputs()`. Remove assertion. Update tests.
6. **`src/commands/new.rs`** — Major: integrate `ForestName`, `BranchName`. Remove `validate_branch_name()`. Update plan structs. Update tests.
7. **`src/commands/rm.rs`** — Update struct types. Update tests.
8. **`src/commands/exec.rs`** — Update `ExecResult`. Update tests.
9. **`src/commands/status.rs`** — Update `StatusResult`, `RepoStatus`. Update tests.
10. **`src/commands/ls.rs`** — Update `ForestSummary`. Update tests.
11. **`src/forest.rs`** — Update comparisons.
12. **Run full suite** — `just check && just test`.

---

## 5. ADR 0010 Update

After both Contracts phases complete, update ADR 0010 to reflect:

**Replace the "Known gap" paragraph with:**

> The original postconditions-only gap has been closed. Three newtypes (`AbsolutePath`, `RepoName`, `ForestName`) eliminate 6 postcondition assertions by making invalid states unrepresentable at compile time. One postcondition remains (`sanitize_forest_name` result has no `/`) as a defense-in-depth check on a pure function. One collection-level postcondition remains (repo name uniqueness) because set membership isn't expressible as a single-value newtype. Two preconditions exist (`cmd_exec`, `cmd_status`: `forest_dir.is_absolute()`) at `&Path` boundaries where the type system doesn't enforce absoluteness. A `BranchName` newtype replaces the `validate_branch_name()` helper in the planning pipeline.

**Update the "Current `debug_assert!` usage" section** to list the final 4 assertions.

---

## 6. Test Strategy

### New Tests

| Test | Location | What it verifies |
|------|----------|-----------------|
| `repo_name_new_valid` | `paths.rs` | `RepoName::new("foo")` succeeds |
| `repo_name_new_empty_fails` | `paths.rs` | `RepoName::new("")` fails |
| `repo_name_serde_round_trip` | `paths.rs` | Serialize/deserialize preserves value |
| `repo_name_deserialize_empty_fails` | `paths.rs` | Deserializing `""` fails |
| `forest_name_new_valid` | `paths.rs` | Normal name succeeds |
| `forest_name_new_empty_fails` | `paths.rs` | Empty string fails |
| `forest_name_new_dot_fails` | `paths.rs` | `"."` and `".."` fail |
| `forest_name_sanitized` | `paths.rs` | `ForestName::new("a/b")?.sanitized() == "a-b"` |
| `forest_name_all_slashes_sanitizes_to_non_empty` | `paths.rs` | `ForestName::new("////")` succeeds, `sanitized()` is `"----"` |
| `branch_name_new_valid` | `paths.rs` | Normal branch name succeeds |
| `branch_name_new_refs_prefix_fails` | `paths.rs` | `refs/heads/main` fails |
| `branch_name_new_remote_prefix_fails` | `paths.rs` | `origin/main` with remote `"origin"` fails |
| `branch_name_new_different_remote_ok` | `paths.rs` | `origin/main` with remote `"upstream"` succeeds (it's not that remote's prefix) |

### Modified Tests

Approximately 40–50 sites across all command test files need mechanical updates:
- `plan.forest_name` comparisons → `plan.forest_name.as_str()`
- `repo.name` comparisons → `repo.name.as_str()`
- Direct struct construction in tests → wrap with `::new().unwrap()`

---

## 7. Risk Assessment

**Low-medium risk.** More pervasive than Contracts 1 because three newtypes are introduced simultaneously, touching more comparison sites. However:
- All changes are mechanical (wrap/unwrap at boundaries).
- The invariants being encoded are simple (non-empty, no bad prefixes).
- The test suite (164+ tests) will catch any missed site immediately — compilation will fail before tests even run.

**The main risk is scope creep:** resisting the urge to also newtype `base_branch`, `remote`, `template_name`, etc. These carry weaker invariants and fewer assertion sites. Defer them unless a bug surfaces.
