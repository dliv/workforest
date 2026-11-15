# v0.8.1

This release delivers the features planned for 0.8.0. That tag failed validation before publishing binaries; 0.8.1 includes the test-fixture identity fix and preserves the original tag.

## Forest status and removal

- Report Git worktree locks and optional reasons in `ls` and `status`, including explicit unavailable observations. ([#5](https://github.com/dliv/workforest/issues/5))
- Preflight every repository before removing a forest, so known blockers preserve the whole forest. This covers locks, unsafe branch deletion, config-hidden untracked content, and submodule checkout/metadata refusals. Preview and live removal share the decision; `rm --all` still handles each forest independently. ([#6](https://github.com/dliv/workforest/issues/6))
- Add structured JSON status: changed-path counts, separate upstream and saved-base comparisons, and `commits_not_on_known_remotes`. Unavailable observations stay distinct from zero; human status output is preserved. ([#7](https://github.com/dliv/workforest/issues/7))
- Include observed lock reasons in removal refusals. Force does not unlock worktrees, and later races or I/O failures can still cause partial cleanup.

Thanks to @fargraph for the workflow reports and clarification behind these changes.

## Agent and contributor documentation

- Expand `git forest agent-instructions` into a self-contained guide to status fields, availability and removal safety.
- Add detailed `--help` for every command and clarify the existing textual exceptions to `--json`.
- Add a concise contribution policy and bug/workflow issue forms. Reports, reproductions and workflow descriptions are welcome; discuss implementation before submitting a PR.
- Clarify development prerequisites, including Python 3.11+ for release-validation tests. The installed CLI does not require Python.

## Dependencies

Update the audited Rust dependencies, including security fixes plus fixes for TLS timeout budgets and TOML serialization. Fresh Rust and worker dependency audits passed before release.

**Upgrade:** `brew update && brew upgrade git-forest`

**Full changelog:** https://github.com/dliv/workforest/compare/v0.7.1...v0.8.1
