# Phase 5 Plan — Review Feedback

Review of `PHASE_5A_PLAN.md` and `PHASE_5B_PLAN.md` against the existing codebase and architecture decisions. Covers the branch config simplification (5A) and multi-template config (5B).

---

## Must-Fix

### 1. `--force` semantics for multi-template init are wrong

Open Question 2 proposes changing `--force` to mean "allow modifying existing config (upsert the template)." This conflates two different operations:

- **Adding a new template to an existing config** — safe, non-destructive.
- **Overwriting an existing template** — destructive, loses the previous template's repos.

Requiring `--force` just to add a second template to an existing config creates unnecessary friction, especially for agents. The first thing you do after `init --template opencop` is `init --template acme` — and you'd need `--force` even though nothing is being overwritten.

**Fix:** `--force` should only be required when overwriting an existing template with the same name. Adding a new template to an existing config should work without `--force`.

```rust
// Pseudocode for the init logic:
let mut config = if config_path.exists() {
    load_config(config_path)?
} else {
    ResolvedConfig { default_template: inputs.template_name.clone(), templates: BTreeMap::new() }
};

if config.templates.contains_key(&inputs.template_name) && !force {
    bail!(
        "template {:?} already exists in config\n  hint: use --force to overwrite, or choose a different name",
        inputs.template_name
    );
}

config.templates.insert(inputs.template_name.clone(), template);
write_config_atomic(config_path, &config)?;
```

This preserves the safety intent (don't silently overwrite) while making the common multi-template setup workflow frictionless.

### 2. `default_template` must be validated

The plan doesn't specify what happens when `default_template` references a template name that doesn't exist in the config. This can happen if:
- The user edits the TOML and typos the default name.
- `init --force` overwrites the template that was the default, and adds a differently-named one.
- The user deletes a template section from the TOML.

**Fix:** Validate during `parse_config()` that `default_template` references an existing key in `templates`. Produce a clear error:

```
error: default_template "opencop" not found in config
  hint: available templates: acme, default
  hint: edit config to set default_template to one of these, or run `git forest init --template opencop ...`
```

Also: `init` should update `default_template` when creating the first template (already covered in the plan), but should NOT change it when adding subsequent templates. The plan's sketch does this correctly, but it's worth calling out as a test case.

### 3. `GeneralConfig` removal creates a cascading refactor bigger than described

The plan says "Remove `GeneralConfig` struct entirely — replaced by `ResolvedTemplate`." But `GeneralConfig` is currently the raw deserialization target for the `[general]` table AND the field type on `ResolvedConfig`. It's used in:

- `config.rs`: `Config.general`, `ResolvedConfig.general`, `parse_config()`, `write_config_atomic()`
- `commands/init.rs`: `validate_init_inputs()` constructs a `GeneralConfig`
- `commands/new.rs`: `plan_forest()` reads `config.general.branch_template`, `config.general.username`, `config.general.worktree_base`
- `main.rs`: `config.general.worktree_base` used by `Ls`, `Rm`, `Status`, `Exec`
- `testutil.rs`: `default_config()` constructs a `GeneralConfig`

The plan's "File-by-File Changes" section correctly identifies all these, but understates the scope. Every callsite that accesses `config.general.worktree_base` needs to change, and there are 5 of them in `main.rs` alone. The plan should call out that `main.rs` is the highest-touch file — all 6 command arms change because the config access pattern fundamentally changes from `config.general.worktree_base` to either `config.all_worktree_bases()` or `config.resolve_template(...)?.worktree_base`.

Not a design bug, but the scope estimate of "~200–300 lines of new/changed code" likely underestimates. Expect ~400–500 including test changes.

### 4. Forest discovery with multiple `worktree_base` paths has an ambiguity problem

The plan proposes `all_worktree_bases()` to collect unique worktree base paths, and searching all of them for `resolve_forest` and `discover_forests`. But:

**What if two templates share the same `worktree_base`?** The plan's `dedup()` handles this for `ls` (don't scan the same directory twice). But for `init`, two templates could point to the same `worktree_base`, and that's fine — forests from different templates coexist in the same directory.

**What if two templates have overlapping but different `worktree_base` paths?** E.g., `~/worktrees` and `~/worktrees/acme`. `discover_forests` scans direct children only (not recursive), so a forest at `~/worktrees/acme/my-feature` would be found by scanning `~/worktrees/acme` but NOT by scanning `~/worktrees` (it would see `acme` as a child, check for `.forest-meta.toml`, not find one, and skip it). This is correct behavior, but the plan doesn't call it out and doesn't test for it.

**Fix:** Add a test case for overlapping worktree bases. Also add a `debug_assert!` that `all_worktree_bases()` returns no path that is a prefix of another (or at least document that nested bases are supported and don't interfere).

### 5. Legacy config backward-compat has a deserialization ambiguity

The plan says: try `MultiTemplateConfig` first, fall back to `LegacyConfig`. But TOML deserialization may succeed for the wrong type. Consider this legacy config:

```toml
[general]
worktree_base = "~/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[repos]]
path = "~/src/foo-api"
```

Deserializing this as `MultiTemplateConfig` will fail because it has `[general]` instead of `[template.*]` and top-level `[[repos]]` instead of `[[template.*.repos]]`. So the fallback works.

But what about a pathological config that happens to parse as both? In practice this can't happen because the structural shapes are completely different (`general` + flat `repos` vs. `default_template` + `template.*`). The plan's approach is sound, but should assert this:

**Fix:** Add a test that a legacy config does NOT accidentally parse as `MultiTemplateConfig`. The plan mentions `parse_legacy_config_auto_converts` but should also have `parse_legacy_config_does_not_parse_as_multi` to document the non-overlap guarantee.

### 6. 5A's `--feature-branch-template` default value is unusable

The plan (5A, cli.rs section) proposes changing the default from `"{user}/{name}"` to `"{name}"`. This means `git forest init --repo ~/src/foo` with no template flag produces `feature_branch_template = "{name}"`, and `git forest new my-feature --mode feature` creates a branch named just `my-feature` with no user prefix.

This is technically correct but practically bad:
- Branch names like `my-feature` will collide with other developers' branches.
- The previous default (`dliv/my-feature`) was useful. The new one requires every user to always pass `--feature-branch-template`.
- Agents will need to be taught to always include the flag, adding friction to the "agent-drivable first" principle.

**Fix:** Require `--feature-branch-template` on `init` (no default), just like `--username` was required before. This makes the user consciously choose their pattern. The error message guides them:

```
error: --feature-branch-template is required
  hint: git forest init --feature-branch-template "yourname/{name}" --repo <path>
```

Alternatively, keep a default of `"{name}"` but emit a warning: `"warning: using default branch template '{name}' — branches won't have a user prefix. Consider --feature-branch-template 'yourname/{name}'"`. Either way, the user needs to be nudged.

---

## Design Decisions to Lock In

### 7. Don't store template name in forest meta — agreed

The plan recommends not adding `template: Option<String>` to `ForestMeta` (Open Question 4). This is correct. Decision 6 ("meta is self-contained") means the meta captures everything needed to operate on the forest. The template name is a creation-time routing decision, not operational state. Adding it would be informational clutter that creates a false expectation that the template matters post-creation.

Lock this in: forests are template-agnostic after creation.

### 8. Auto-detect legacy format — do it, but with a clearer migration path

Open Question 1 asks whether to auto-detect or error. Auto-detect is the right call (~20 lines, friendlier), but the plan's stderr warning is passive. Improve it:

```
warning: config uses legacy format (will be unsupported in a future version)
  hint: run `git forest init --force --template default --repo <path1> --repo <path2> ...` to upgrade
```

The hint should be actionable — tell the user the exact command, not just "run init." Since you have the parsed config, you could even generate the exact command with their repos. That's a nice-to-have, not a must-fix.

### 9. `--default-template` flag — defer, but document the manual edit

Open Question 3 asks about a `--default-template` flag on init. Defer is correct — changing the default template is rare. But document it in `--help` or the README:

```
To change the default template, edit ~/.config/git-forest/config.toml:
  default_template = "acme"
```

### 10. `resolve_template` helper — good, but handle the empty-config edge case

The `resolve_template` helper is clean. But what if the config has zero templates? This shouldn't happen in normal use (init creates at least one), but a hand-edited config could be empty. The error from `templates.get(key)` would say `template "default" not found` which is confusing when the real problem is "no templates at all."

Add a guard:

```rust
if self.templates.is_empty() {
    bail!("no templates configured\n  hint: run `git forest init --repo <path> ...` to create one");
}
```

### 11. `write_config_atomic` — remove `force` parameter

The plan correctly notes that `force` logic should move to `cmd_init`. `write_config_atomic` should be a pure "serialize and write" function. The caller decides whether writing is allowed. This is cleaner and matches the plan/execute pattern.

Currently `write_config_atomic` takes `(path, config, force)` — after 5B, it takes `(path, config)` and always writes. The `force`/existence check lives in `cmd_init`.

---

## Minor Items

- **5A execution order matters.** 5A removes `username` and renames `branch_template`. If these land as one commit, the test change surface is large but mechanical. If split across commits, there's a window where the code doesn't compile. Recommend: land 5A as a single commit, not incremental steps.

- **5B's `BTreeMap` choice is good.** Deterministic ordering means config files serialize identically across runs. The plan calls this out, but it's worth reinforcing — `HashMap` would produce non-deterministic TOML output.

- **Template name validation.** Template names should be non-empty and not contain characters that would break TOML table syntax. `[template.foo bar]` is valid TOML (quoted keys), but `[template.]` is not. Add a simple validation: non-empty, no leading/trailing whitespace. Don't over-validate — TOML handles most cases.

- **5B test count estimate is low.** The plan says "~12 new tests." Given the scope of config parsing changes (two formats, fallback, multi-base scanning, template resolution, init upsert logic), expect 18–20 new tests. The plan's list of 12 is a minimum.

- **`init` result struct needs a template name field.** After 5B, `InitResult` should include `template_name: String` so JSON consumers know which template was created/updated. The plan doesn't mention this.

- **5A migration hint is cheap, do it.** The plan asks whether to detect old `branch_template` + `username` fields and suggest the new format. Yes — it's ~5 lines in `parse_config()` error handling and prevents user confusion. The tool is pre-v1, but you (the developer) have an existing config that will break.

- **Phase ordering between 5A and 5B.** The plan says 5A must complete before 5B. This is correct — 5B's config schema builds on the simplified `feature_branch_template` field from 5A. But Phase 3 (if not yet landed) should land first, because 5A changes `compute_target_branch` which is introduced in Phase 3. Verify the dependency chain: Phase 3 → 5A → 5B.
