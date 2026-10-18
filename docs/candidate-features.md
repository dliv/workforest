# Candidate features

A feature is a noun git-forest owns that passes the feature-area test:

- it has a lifecycle that outlives one run of the tool;
- another party reads it: a person, another tool, or another git-forest command;
- it is not the tool's own settings.

A feature gets its own crates under `crates/<feature>/<role>/` (ADR 0017). Each verdict below is dated and names the trigger that would change it, so a later reader can check whether the trigger has fired. Add a dated entry when a verdict changes or a new candidate appears; do not rewrite old entries.

## 2026-10-18

- **The forest is the feature.** It is the directory, its metadata file, its worktrees and the branches `new` creates. `new` creates it, `rm` and `reset` transition it, and `ls`, `status` and `exec` read it.
- **Templates are not a feature today.** `ResolvedTemplate` is the forest planner's input type, defined in `crates/forest/core/src/plan/new.rs`, and `init` validates and writes a forest template. Trigger: a command that manages templates as their own objects, such as listing, inspecting or deleting one.
- **The update check is the edge**, a client of the update server. **Git is a shared client** that every part uses. **`config.toml` is the tool's own settings.**
