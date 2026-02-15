# Phase 5B Plan — Multi-Template Config

**STATUS: PLANNED** — Design questions flagged inline. Ready for execution after 5A is complete and open questions are resolved.

## Goal

Support multiple named repo groups ("templates") so a developer can manage more than one faux-monorepo on a single machine. Currently the config is a singleton — one set of repos, one `worktree_base`. This is like git only allowing one repo per machine.

## Prerequisite

Phase 5A must be complete first. 5B builds on the simplified config (no `username`, `feature_branch_template` instead of `branch_template`).

---

## Config Schema

### New Format

```toml
default_template = "opencop"

[template.opencop]
worktree_base = "~/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "~/src/opencop-java"

[[template.opencop.repos]]
path = "~/src/opencop-web"
base_branch = "main"

[template.acme]
worktree_base = "~/worktrees/acme"
base_branch = "main"
feature_branch_template = "dliv/{name}"

[[template.acme.repos]]
path = "~/src/acme-api"

[[template.acme.repos]]
path = "~/src/acme-web"
```

### Raw Deserialization Structs

```rust
use std::collections::BTreeMap;  // deterministic key ordering

#[derive(Debug, Serialize, Deserialize)]
pub struct MultiTemplateConfig {
    pub default_template: String,
    pub template: BTreeMap<String, TemplateConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub repos: Vec<RepoConfig>,
}
```

`RepoConfig` stays as-is (from Phase 5A): `path`, optional `name`, optional `base_branch`, optional `remote`.

The `toml` crate (0.8) supports `BTreeMap<String, TemplateConfig>` natively — `[template.opencop]` tables deserialize into map entries, and `[[template.opencop.repos]]` arrays of tables deserialize into `Vec<RepoConfig>` on the corresponding entry. Verified empirically.

### Resolved Types (Post-Parse)

`ResolvedConfig` changes:

```rust
// BEFORE (after 5A)
pub struct ResolvedConfig {
    pub general: GeneralConfig,
    pub repos: Vec<ResolvedRepo>,
}

// AFTER
pub struct ResolvedConfig {
    pub default_template: String,
    pub templates: BTreeMap<String, ResolvedTemplate>,
}

pub struct ResolvedTemplate {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub repos: Vec<ResolvedRepo>,
}
```

`ResolvedRepo` stays unchanged: `path`, `name`, `base_branch`, `remote`.

### Helper: Resolve a Single Template

Most consumers need one template at a time. Add a helper:

```rust
impl ResolvedConfig {
    pub fn resolve_template(&self, name: Option<&str>) -> Result<&ResolvedTemplate> {
        if self.templates.is_empty() {
            bail!("no templates configured\n  hint: run `git forest init --repo <path> ...` to create one");
        }
        let key = name.unwrap_or(&self.default_template);
        self.templates.get(key).ok_or_else(|| {
            let available: Vec<&str> = self.templates.keys().map(|k| k.as_str()).collect();
            anyhow!("template {:?} not found\n  hint: available templates: {}", key, available.join(", "))
        })
    }
}
```

---

## Backward Compatibility

No legacy format support needed. The tool is pre-v1 with a single user. Delete old config and re-run `git forest init`.

The old `Config` struct from 5A can be removed entirely — no `LegacyConfig` rename, no fallback parsing.

**Validation:** `default_template` must reference an existing key in `templates`. Check during `parse_config()`:

```rust
if !resolved.templates.contains_key(&resolved.default_template) {
    let available: Vec<&str> = resolved.templates.keys().map(|k| k.as_str()).collect();
    bail!(
        "default_template {:?} not found in config\n  hint: available templates: {}\n  hint: edit config to fix default_template, or run `git forest init --template {} ...`",
        resolved.default_template,
        available.join(", "),
        resolved.default_template
    );
}
```

---

## Command Changes

### `new` — Add `--template` Flag

```rust
// cli.rs, Command::New
/// Template to use (default: from config's default_template)
#[arg(long)]
template: Option<String>,
```

**In `main.rs`:**

```rust
Command::New { name, mode, template, branch, repo_branches, no_fetch, dry_run } => {
    let config = config::load_default_config()?;
    let tmpl = config.resolve_template(template.as_deref())?;
    // ... build NewInputs, call cmd_new(inputs, tmpl)
}
```

**In `cmd_new` / `plan_forest`:** These currently take `&ResolvedConfig`. After this change, they take `&ResolvedTemplate` instead — they only need one template's worth of data (worktree_base, feature_branch_template, repos).

This is a clean narrowing of the interface: `plan_forest` never needed the full config, it just happened to receive it. After 5B, it receives exactly what it needs.

### `init` — Add `--template` Flag, Per-Template Updates

```rust
// cli.rs, Command::Init
/// Template name to create or update (default: "default")
#[arg(long, default_value = "default")]
template: String,
```

**Behavior:**

- `git forest init --template opencop --repo ~/src/a --repo ~/src/b --feature-branch-template "dliv/{name}"`
  → Creates/replaces the `opencop` template in the config.
- If the config doesn't exist, creates it with `default_template` set to this template name.
- If the config exists, adds the new template. Other templates are preserved.
- `--force` is only required when overwriting an existing template *by the same name*.
- When adding a subsequent template, `default_template` is NOT changed (it stays pointing at the first template created).

**DECIDED: `--force` means "overwrite an existing template."** Adding a new template to an existing config is safe and non-destructive — it should work without `--force`. Only overwriting an existing template name requires `--force`, since that loses the previous template's repos.

**Implementation sketch:**

```rust
pub fn cmd_init(inputs: InitInputs, config_path: &Path, force: bool) -> Result<InitResult> {
    let template = validate_init_inputs(&inputs)?;  // Returns ResolvedTemplate

    let mut config = if config_path.exists() {
        load_config(config_path)?
    } else {
        ResolvedConfig {
            default_template: inputs.template_name.clone(),
            templates: BTreeMap::new(),
        }
    };

    // Only require --force when overwriting an existing template
    if config.templates.contains_key(&inputs.template_name) && !force {
        bail!(
            "template {:?} already exists in config\n  hint: use --force to overwrite, or choose a different name",
            inputs.template_name
        );
    }

    config.templates.insert(inputs.template_name.clone(), template);
    write_config_atomic(config_path, &config)?;
    // ...
}
```

**DECIDED: Defer `--default-template` flag.** Changing the default template is rare. Document the manual edit in README:

```
To change the default template, edit ~/.config/git-forest/config.toml:
  default_template = "acme"
```

### `ls`, `rm`, `status`, `exec` — No `--template` Needed

These commands work from forest meta (`.forest-meta.toml`), which is self-contained (Decision 6). They don't read the global config for anything except `worktree_base` (to find forests).

**However**, `worktree_base` is now per-template. This affects `ls` and `rm`/`status`/`exec` resolution:

- **`ls`**: Currently scans one `worktree_base`. With multi-template, it needs to scan all unique `worktree_base` directories across all templates.
- **`rm`/`status`/`exec`**: Use `resolve_forest(worktree_base, name)`. Need to search across all `worktree_base` directories.

**Approach:** Add a helper that collects all unique `worktree_base` paths from the config:

```rust
impl ResolvedConfig {
    pub fn all_worktree_bases(&self) -> Vec<&Path> {
        let mut bases: Vec<&Path> = self.templates.values()
            .map(|t| t.worktree_base.as_path())
            .collect();
        bases.sort();
        bases.dedup();
        bases
    }
}
```

Then `cmd_ls` scans all bases. `resolve_forest` searches all bases.

**DECIDED: Don't store template name in forest meta.** Forests are template-agnostic after creation (Decision 6). The meta captures everything needed to operate on the forest. The template name is a creation-time routing decision, not operational state. Adding it would be informational clutter that creates a false expectation that the template matters post-creation.

---

## `write_config_atomic` Changes

Currently writes from `ResolvedConfig` by converting back to `Config` (the raw serde struct). After 5B, it writes from `ResolvedConfig` by converting back to `MultiTemplateConfig`:

```rust
pub fn write_config_atomic(path: &Path, config: &ResolvedConfig) -> Result<()> {
    let raw = MultiTemplateConfig {
        default_template: config.default_template.clone(),
        template: config.templates.iter().map(|(name, tmpl)| {
            (name.clone(), TemplateConfig {
                worktree_base: tmpl.worktree_base.clone(),
                base_branch: tmpl.base_branch.clone(),
                feature_branch_template: tmpl.feature_branch_template.clone(),
                repos: tmpl.repos.iter().map(|r| RepoConfig {
                    path: r.path.clone(),
                    name: Some(r.name.clone()),
                    base_branch: Some(r.base_branch.clone()),
                    remote: Some(r.remote.clone()),
                }).collect(),
            })
        }).collect(),
    };
    // ... same atomic write pattern
}
```

**DECIDED: Remove `force` parameter from `write_config_atomic`.** It becomes a pure "serialize and write" function: `write_config_atomic(path, config) -> Result<()>`. The caller (`cmd_init`) decides whether writing is allowed. This matches the plan/execute pattern — the decision logic lives in the command, the write function just writes.

---

## File-by-File Changes

### `src/config.rs` — Major Rewrite

| Item | Change |
|------|--------|
| `Config` struct | Remove (replaced by `MultiTemplateConfig`) |
| New `MultiTemplateConfig` struct | Add for raw deserialization |
| New `TemplateConfig` struct | Add for per-template raw deserialization |
| `GeneralConfig` struct | Remove entirely — replaced by `ResolvedTemplate` |
| `ResolvedConfig` struct | Replace `general` + `repos` with `default_template` + `templates: BTreeMap` |
| New `ResolvedTemplate` struct | `worktree_base`, `base_branch`, `feature_branch_template`, `repos: Vec<ResolvedRepo>` |
| `parse_config()` | Rewrite: try new format, fall back to legacy, resolve either way |
| `write_config_atomic()` | Rewrite: serialize from new `ResolvedConfig` to `MultiTemplateConfig` |
| All tests | Update to use new config format in TOML strings |

### `src/commands/init.rs` — Significant Changes

| Item | Change |
|------|--------|
| `InitInputs` | Add `template_name: String` field |
| `InitResult` | Add `template_name: String` field (for JSON consumers) |
| `validate_init_inputs()` | Returns `ResolvedTemplate` instead of `ResolvedConfig` |
| `cmd_init()` | Load existing config (if any), upsert template, write. `--force` only required when overwriting existing template name. |
| Tests | Update for new flow |

### `src/commands/new.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `plan_forest()` | Takes `&ResolvedTemplate` instead of `&ResolvedConfig` |
| `cmd_new()` | Takes `&ResolvedTemplate` instead of `&ResolvedConfig` |
| `compute_target_branch()` | No change (already takes individual fields) |
| Tests | Update config construction to use `ResolvedTemplate` |

### `src/commands/ls.rs` — Minor Changes

| Item | Change |
|------|--------|
| `cmd_ls()` | Takes `&[&Path]` (multiple worktree bases) instead of single `&Path` |

### `src/commands/status.rs`, `exec.rs`, `rm.rs` — No Logic Changes

These work from `ForestMeta` via `resolve_forest()`. The only change is in `main.rs` where `resolve_forest` is called — it needs to search multiple `worktree_base` paths.

### `src/forest.rs` — Minor Changes

| Item | Change |
|------|--------|
| `discover_forests()` | Add variant that takes multiple bases, or call existing one per base and merge |
| `resolve_forest()` | Search across multiple bases |

### `src/cli.rs` — Minor Changes

| Item | Change |
|------|--------|
| `Command::Init` | Add `--template` flag |
| `Command::New` | Add `--template` flag |

### `src/main.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `Command::Init` arm | Pass template name through, handle per-template upsert |
| `Command::New` arm | Resolve template, pass `&ResolvedTemplate` to `cmd_new` |
| `Command::Ls` arm | Pass all worktree bases |
| `Command::Rm/Status/Exec` arms | Search all worktree bases for forest resolution |

### `src/testutil.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `default_config()` | Returns new `ResolvedConfig` with one template named `"default"` |
| Possibly add `default_template()` | Returns a `ResolvedTemplate` directly for tests that only need one |

### `tests/cli_test.rs` — Minor Changes

Integration tests that use `init` don't pass `--template`, so they get the default. The `setup_new_env` helper may need a `--template` in some new tests but existing tests should work as-is.

---

## Execution Order

1. **`src/config.rs`** — New structs, parse logic, backward compat. This is the foundation.
2. **`src/testutil.rs`** — Update helpers so other test files compile.
3. **`src/commands/init.rs`** — New init flow with template upsert.
4. **`src/commands/new.rs`** — Narrow interface to `ResolvedTemplate`.
5. **`src/commands/ls.rs`** — Multi-base scanning.
6. **`src/forest.rs`** — Multi-base forest resolution.
7. **`src/main.rs`** — Wire everything together.
8. **`src/cli.rs`** — Add `--template` flags.
9. **`tests/cli_test.rs`** — Update integration tests.
10. **Documentation** — Update README, architecture-decisions.

Each step should be a passing `just check && just test` checkpoint.

---

## Resolved Design Decisions

All open questions from the original plan have been resolved:

| # | Question | Decision |
|---|----------|----------|
| 1 | Auto-detect legacy config or error? | **Neither.** No legacy support needed. Pre-v1, single user. Delete old config and re-run init. |
| 2 | What does `--force` mean for init? | **`--force` = overwrite existing template by same name.** Adding a new template to existing config works without `--force`. |
| 3 | Add `--default-template` flag? | **Defer.** Editing TOML is fine for this rare operation. Document in README. |
| 4 | Store template name in forest meta? | **No.** Forests are template-agnostic after creation (Decision 6). |

### Additional Design Points (from review)

- **Validate `default_template`:** `parse_config()` must verify `default_template` references an existing template key. Clear error with available names.
- **Empty config guard:** `resolve_template()` checks for empty templates map before lookup, with a "run `git forest init`" hint.
- **Template name validation:** Template names must be non-empty and have no leading/trailing whitespace. Don't over-validate — TOML handles most cases.
- **`InitResult` gets `template_name` field:** JSON consumers need to know which template was created/updated.
- **Overlapping worktree bases:** If two templates share the same `worktree_base`, `dedup()` prevents double-scanning. If bases are nested (e.g., `~/worktrees` and `~/worktrees/acme`), `discover_forests` scans direct children only so they don't interfere. Add a test for this edge case.
- **Remove `force` from `write_config_atomic`:** Force logic moves to `cmd_init`. Write function is pure serialize-and-write.

---

## Test Strategy

### New Tests Needed

**Config parsing:**
- `parse_multi_template_config` — two templates, verify both parsed correctly.
- `parse_multi_template_default_resolution` — verify `resolve_template(None)` uses default.
- `parse_multi_template_explicit_resolution` — verify `resolve_template(Some("acme"))` works.
- `parse_multi_template_unknown_template_errors` — helpful error with available names.
- `parse_multi_template_empty_templates_errors` — empty config guard in `resolve_template`.
- `parse_multi_template_invalid_default_errors` — `default_template` references nonexistent key.
- `template_name_validation` — empty name errors, whitespace-trimmed name errors.

**Init:**
- `init_creates_new_template` — first template in fresh config.
- `init_adds_second_template_without_force` — adding new template to existing config works.
- `init_overwrites_existing_template_requires_force` — overwrite same name without `--force` errors.
- `init_replaces_existing_template_with_force` — overwrite a template, others preserved.
- `init_first_template_becomes_default` — `default_template` set on first init.
- `init_second_template_does_not_change_default` — `default_template` unchanged when adding.
- `init_result_includes_template_name` — JSON output has `template_name` field.

**New:**
- `new_with_explicit_template` — `--template acme` uses acme's repos.
- `new_default_template` — no `--template` flag uses default.

**Forest discovery:**
- `ls_scans_multiple_worktree_bases` — forests from different templates both appear.
- `resolve_forest_across_bases` — forest found regardless of which template's base it's in.
- `overlapping_worktree_bases_no_double_scan` — nested bases don't cause duplicate results.

### Existing Tests

Most existing unit tests construct `ResolvedConfig` directly. These need to be updated to use the new struct shape (wrapping existing data in a template). The changes are mechanical but widespread.

### Integration Tests

The `setup_new_env` helper creates config via `git forest init` CLI. After 5B, this creates a single template named "default". All existing integration tests should pass without changes since they don't use `--template`.

New integration tests:
- `init_with_template_name` — creates named template.
- `new_with_template_flag` — uses non-default template.

---

## Estimated Scope

- **~400–500 lines of new/changed code** across config.rs, init.rs, new.rs, main.rs, forest.rs. (Revised up from initial estimate — `main.rs` is the highest-touch file since all 6 command arms change from `config.general.worktree_base` to the new access patterns.)
- **~200 lines of test changes** (mostly mechanical struct construction updates).
- **~19 new tests.**
- **Net: significant but contained.** The key insight is that most commands don't care about templates — they work from forest meta. The template concept only affects `init` (writes config), `new` (reads config), and `ls`/`resolve_forest` (needs to know where to scan).
