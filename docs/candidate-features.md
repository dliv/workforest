# Candidate features

A feature groups git-forest's work that fulfills a cohesive responsibility for the user or an agent driving the tool: the decisions, workflows and outcomes that responsibility needs, whether they change state, compute a result or report data. Extend an existing feature when new work extends its responsibility. Create a new feature only when adopted behavior has a distinct responsibility that no existing feature owns; then state its consumer, the outcome and guarantees the tool owns, and why the nearest existing feature is not its owner, and name it with the tool's own vocabulary. A new command, noun, file, cache or client alone does not establish a feature. Shared clients (git), the composition root and the tool's own settings are not features.

A feature gets its own crates under `crates/<feature>/<role>/` (ADR 0017). Each verdict below is dated and names what would prompt a placement review, so a later reader can check whether that has happened. Add a dated entry when a verdict changes or a new candidate appears; do not rewrite old entries.

## 2026-10-18

- **The forest is the feature.** Its responsibility is creating, tending, inspecting and tearing down forests: the directory, its metadata file, its worktrees and the branches `new` creates. `new` creates it, `rm` and `reset` transition it, and `ls`, `status` and `exec` read it.
- **Templates extend the forest responsibility.** `init` configures how forests are made, and `ResolvedTemplate` is the forest planner's input type (`crates/forest/core/src/plan/new.rs`). Review prompt: a command that lists, inspects or deletes templates calls for a placement review (does it extend the forest responsibility, or form a distinct one with its own consumer and guarantees?), recorded here as a dated entry.
- **The update check is the edge**, a client of the update server. **Git is a shared client** that every part uses. **`config.toml` is the tool's own settings.**
