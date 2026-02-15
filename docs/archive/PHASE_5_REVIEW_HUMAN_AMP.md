# Phase 5 — Config Architecture Decision: Single File vs. Per-Template Files

Follow-up to `PHASE_5_REVIEW_AMP.md` item #1 (`--force` semantics). Evaluates whether splitting templates into separate files would be a better design.

---

## Question

The `--force` friction in 5B's `init` exists because everything lives in one config file. Would it be better to use one file per template?

- **Option A (current plan):** Single `~/.config/git-forest/config.toml` with `BTreeMap<String, TemplateConfig>`.
- **Option B:** Root config (`config.toml`) holds only `default_template`. Each template lives in `~/.config/git-forest/templates/<name>.toml`.

## Decision: Option A (single file), with `--force`-only-on-overwrite fix

Option B is attractive for per-template ergonomics but doesn't justify the added complexity. The `--force` friction that motivated the question is solved by a simpler fix: only require `--force` when overwriting an existing template name, not when adding a new one to the config.

## Evaluation

### Where Option A wins

1. **Parsing stays trivial.** One `toml::from_str` call. Option B requires directory scanning, N-file parsing, merging results, handling parse errors with "which file failed" context.

2. **Atomic writes are free.** Single temp-file + rename. Option B needs either multi-file transactional logic or accepts partial state (root updated but template file not, or vice versa).

3. **Fewer edge cases.** No stale template files, no editor swap files (`.swp`, `~`) in the templates directory, no filename-vs-internal-name mismatches, no "root says default is X but `templates/X.toml` is missing."

4. **Agent-drivable (Decision 7).** One file to read/patch. Agents can do deterministic edits on a single document. Option B adds procedural steps and more chances for partial state.

5. **Backward compatibility is cleaner.** Legacy format detection is one `toml::from_str` fallback. Option B would need to handle "legacy config in root file" plus "new templates in directory" simultaneously.

6. **Less runtime IO.** Any command that loads config reads one file. Option B reads 1 + N files.

### Where Option B wins

1. **Per-template human editing.** Each template file is flat and simple — no TOML dotted table nesting like `[template.opencop]` / `[[template.opencop.repos]]`.

2. **Template isolation.** `init --template acme` writes only `templates/acme.toml` — no read-modify-write of a shared file. No `--force` question at all.

3. **Future import/export.** If we ever want `git forest template export/import`, file-per-template is the natural unit.

### Why Option B doesn't justify itself now

- The `--force` friction is solved more simply: only require it when overwriting an existing template name.
- The human editing advantage is marginal — the `[template.name]` nesting is slightly verbose but readable.
- The import/export use case is speculative and post-v1.
- The multi-file complexity (scanning, atomicity, edge cases) is real cost for speculative benefit.

## When to revisit Option B

- Users maintain many templates and complain about config file size.
- Template sharing/distribution becomes a feature (`git forest template import/export`).
- We're willing to implement multi-file consistency safeguards.

## Impact on PHASE_5_REVIEW_AMP.md

This decision reinforces review item #1's fix. The `init` logic becomes:

```rust
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
```

No other changes to the 5B plan are needed from this decision.
