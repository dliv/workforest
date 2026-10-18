# 17. Crate Layout

Date: 2026-10-18
Status: Accepted

## Context

The policy crate (ADR 0016) keeps effects and effectful imports out of the Pure modules with a token scanner. A scanner reads names: an unusual spelling can slip past it, and while the Pure modules share one package with the effectful code, it cannot stop a Pure module from naming a crate that package depends on, such as the HTTP client. Separate crates let the compiler and Cargo enforce the direction instead: a crate cannot name a crate missing from its manifest, and a dependency cycle does not build.

## Decision

Each role is its own workspace member, and a feature's crates live at `crates/<feature>/<role>/`. The end state:

- `crates/forest/core` (`git-forest-core`), Pure: the forest's decisions, types and renderers, meaning the path newtypes, the forest metadata types, and the plan, result and renderer code of `new`, `rm` and `reset`. Present.
- `crates/forest/ops` (`git-forest-ops`), Effectful: the forest's observe and execute steps, the git wrapper, forest discovery and the metadata file's I/O. To follow.
- `crates/update-check` (`git-forest-update-check`), Edge: the update server client, its state file and `VersionResponse`. To follow.
- The repository root (`git-forest`), Binary: `src/lib.rs` and the binaries, `cli`, `channel` and configuration resolution; the composition root.
- `crates/policy` (`git-forest-policy`), Tooling: the architecture checks.

Reasons:

- The root stays the binary package because `.github/workflows/release.yml` builds with `--features` and `--bin` at the repository root, which needs a package there.
- Every other member is `version = "0.0.0"` and `publish = false`, and is named by a path dependency without a `version` key, so `just release`'s version `sed` and `CARGO_PKG_VERSION` stay with the root package. For the same reason `cli`, which reads the version, and `channel`, which with `cli` branches on the stable and beta features, stay in the root; the core needs neither features nor a version.
- The forest is the one feature. By the feature-area test (`docs/candidate-features.md`), a feature is a noun the tool owns, with a lifecycle that outlives one run, that another party reads, and that is not the tool's own settings. The forest is the directory, its metadata file, its worktrees and the branches `new` creates; `new` creates it, `rm` and `reset` transition it, and `ls`, `status` and `exec` read it. The update check is an edge, one client of one outside protocol; git is a client every part shares; `config.toml` is the tool's own settings.
- `ops` names the effect, so the crate's class shows in its path. `update-check` is flat and named for what it is, because it is one outbound client, not a feature with roles.

## Consequences

- Root code names the core directly (`git_forest_core::paths::...`), and the root does not re-export the core's modules at their old paths, so `crate::` always means the crate the file is in. A path into another workspace crate is not an edge of the direction scan; the edges test checks the crate dependency instead (`docs/architecture.md`, "Crate classes and edges").
- Reading and writing the metadata file and reading a worktree's branch state are free functions in the root package (`read_meta`, `write_meta`, `read_branch_state`), because an inherent `impl` must live in the crate that defines its type.
- The edges test gates the crate graph: every member has a class, and a Pure crate depends only on Pure crates of its feature and on the allowed external crates at their pinned features.
- The stable and beta features and the version stay in the root package.
- ts-rs is untouched while `version_check` stays in the root package; moving it into `crates/update-check` changes its `export_to` path.
- `Cargo.lock` changes once for each new member, and that change is committed with it.
- Where `init` and the configuration file's I/O live stays open until the effects crate is designed: the config file is written by `init` and backed up by `reset`, while configuration is resolved at the composition root.

Revisit if: a second noun passes the feature-area test, or a second composition root appears.
