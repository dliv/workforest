# 17. Crate Layout

Date: 2026-10-18
Status: Accepted

## Context

The policy crate (ADR 0016) keeps effects and effectful imports out of the Pure modules with a token scanner. A scanner reads names: an unusual spelling can slip past it, and while the Pure modules share one package with the effectful code, it cannot stop a Pure module from naming a crate that package depends on, such as the HTTP client. Separate crates let the compiler and Cargo enforce the direction instead: a crate cannot name a crate missing from its manifest, and a dependency cycle does not build.

## Decision

Each role is its own workspace member, and a feature's crates live at `crates/<feature>/<role>/`. The end state:

- `crates/forest/core` (`git-forest-core`), Pure: the forest's decisions, types and renderers, meaning the path newtypes, the forest metadata types, and the plan, result and renderer code of `new`, `rm` and `reset`. Present.
- `crates/forest/ops` (`git-forest-ops`), Effectful: the forest's observe and execute steps, the git wrapper, forest discovery and the metadata file's I/O. To follow.
- `crates/update-check` (`git-forest-update-check`), Edge: the update server client, its state file and `VersionResponse`. Present.
- The repository root (`git-forest`), Binary: `src/lib.rs` and the binaries, `cli`, `channel` and configuration resolution; the composition root.
- `crates/policy` (`git-forest-policy`), Tooling: the architecture checks.

Reasons:

- The root stays the binary package because `.github/workflows/release.yml` builds with `--features` and `--bin` at the repository root, which needs a package there.
- Every other member is `version = "0.0.0"` and `publish = false`, and is named by a path dependency without a `version` key, so `just release`'s version `sed` and `CARGO_PKG_VERSION` stay with the root package. For the same reason `cli`, which reads the version, and `channel`, which with `cli` branches on the stable and beta features, stay in the root; the core needs neither features nor a version.
- The forest is the one feature. A feature groups the tool's work that fulfills a cohesive responsibility for the user or an agent driving it; `docs/candidate-features.md` states the rule and records each verdict. The forest is that responsibility: the directory, its metadata file, its worktrees and the branches `new` creates; `new` creates it, `rm` and `reset` transition it, and `ls`, `status` and `exec` read it. The update check is an edge, one client of one outside protocol; git is a client every part shares; `config.toml` is the tool's own settings; `init`, which configures how forests are made, extends the forest responsibility.
- `ops` names the effect, so the crate's class shows in its path. `update-check` is flat and named for what it is, because it is one outbound client, not a feature with roles.

## Consequences

- Root code names the core directly (`git_forest_core::paths::...`), and the root does not re-export the core's modules at their old paths, so `crate::` always means the crate the file is in. A path into another workspace crate is not an edge of the direction scan; the edges test checks the crate dependency instead (`docs/architecture.md`, "Crate classes and edges").
- Reading and writing the metadata file and reading a worktree's branch state are free functions in the root package (`read_meta`, `write_meta`, `read_branch_state`), because an inherent `impl` must live in the crate that defines its type.
- The edges test gates the crate graph: every member has a class, and a Pure crate depends only on Pure crates of its feature and on the allowed external crates at their pinned features.
- The stable and beta features and the version stay in the root package. The update check receives the version, the app and command names, the channel, the state file's path and the enabled flag as one settings value built by the root, with the update server's endpoint and the internal background-check argument as its own constants.
- ts-rs: `VersionResponse`'s `export_to` keeps `worker/src/generated/`, and the ignored export test anchors its output directory at the repository root (`with_out_dir` from the crate's manifest directory), so it writes the tracked binding from any directory and without `TS_RS_EXPORT_DIR`. `ts(export)` is gone, so a plain `cargo test` writes no binding, and `just generate-types` names the package and fails when no export test ran.
- `Cargo.lock` changes once for each new member, and that change is committed with it.
- Where `init` and the configuration file's I/O live stays open until the effects crate is designed: the config file is written by `init` and backed up by `reset`, while configuration is resolved at the composition root.

Revisit if: a placement review finds a second responsibility, or a second composition root appears.
