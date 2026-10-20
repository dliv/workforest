# 17. Crate Layout

Date: 2026-10-18
Status: Accepted

## Context

The policy crate (ADR 0016) keeps effects and effectful imports out of the Pure modules with a token scanner. A scanner reads names: an unusual spelling can slip past it, and while the Pure modules share one package with the effectful code, it cannot stop a Pure module from naming a crate that package depends on, such as the HTTP client. Separate crates let the compiler and Cargo enforce the direction instead: a crate cannot name a crate missing from its manifest, and a dependency cycle does not build.

## Decision

Each role is its own workspace member, and a feature's crates live at `crates/<feature>/<role>/`. The end state:

- `crates/forest/core` (`git-forest-core`), Pure: the forest's decisions, types and renderers, meaning the path newtypes, the forest metadata types, the decide steps of `new`, `rm`, `reset`, `ls` and `init`, and the result types and renderers of every command. Present.
- `crates/forest/ops` (`git-forest-ops`), Effectful: the forest's observe and execute steps, the git wrapper, forest discovery, the metadata file's I/O, and the configuration file's I/O (`config`), which `init` writes and `reset` backs up. Present.
- `crates/update-check` (`git-forest-update-check`), Edge: the update server client, its state file and `VersionResponse`. Present.
- The repository root (`git-forest`), Binary: `src/lib.rs` and the binaries, `cli`, `channel`, reading the environment and the working directory once, resolving the file locations and loading the configuration through ops; the composition root.
- `crates/policy` (`git-forest-policy`), Tooling: the architecture checks.

Reasons:

- The root stays the binary package because `.github/workflows/release.yml` builds with `--features` and `--bin` at the repository root, which needs a package there.
- Every other member is `version = "0.0.0"` and `publish = false`, and is named by a path dependency without a `version` key, so `just release`'s version `sed` and `CARGO_PKG_VERSION` stay with the root package. For the same reason `cli`, which reads the version, and `channel`, which with `cli` branches on the stable and beta features, stay in the root; the core and ops need neither features nor a version, and the update check receives the version as data.
- The forest is the one feature. A feature groups the tool's work that fulfills a cohesive responsibility for the user or an agent driving it; `docs/candidate-features.md` states the rule and records each verdict. The forest is that responsibility: the directory, its metadata file, its worktrees and the branches `new` creates; `new` creates it, `rm` and `reset` transition it, and `ls`, `status` and `exec` read it. The update check is an edge, one client of one outside protocol; git is a client every part shares; `config.toml` is the tool's own settings; `init`, which configures how forests are made, extends the forest responsibility.
- `ops` names the effect, so the crate's class shows in its path. `update-check` is flat and named for what it is, because it is one outbound client, not a feature with roles.

## Consequences

- Code names other workspace crates directly (`git_forest_core::...`, `git_forest_ops::...`), and no crate re-exports another's modules, so `crate::` always means the crate the file is in. A path into another workspace crate is not an edge of the direction scan; the edges test checks the crate dependency instead (`docs/architecture.md`, "Crate classes and edges").
- Reading and writing the metadata file and reading a worktree's branch state are free functions in the forest ops crate (`read_meta`, `write_meta`, `read_branch_state`), because an inherent `impl` must live in the crate that defines its type.
- The edges test gates the crate graph: every member has a class; a Pure crate depends only on Pure crates of its feature and on the allowed external crates at their pinned features; an Effectful crate depends only on Pure and Effectful crates of its feature, and an Edge crate on no member, each on the external crates its class's allowlist names; an allowlist entry no crate of the class declares fails the test.
- The stable and beta features and the version stay in the root package. The update check receives the version, the app and command names, the channel, the state file's path and the enabled flag as one settings value built by the root, with the update server's endpoint and the internal background-check argument as its own constants.
- ts-rs: `VersionResponse`'s `export_to` keeps `worker/src/generated/`, and the ignored export test anchors its output directory at the repository root (`with_out_dir` from the crate's manifest directory), so it writes the tracked binding from any directory and without `TS_RS_EXPORT_DIR`. `ts(export)` is gone, so a plain `cargo test` writes no binding, and `just generate-types` names the package and fails when no export test ran.
- `Cargo.lock` changes once for each new member, and that change is committed with it.
- Where `init` and the configuration file's I/O live is decided: the config module lives in the ops crate, and the root resolves the config and state file locations once, from the environment it read and the channel's app name, and passes them on: `init` the config path and the home, `reset` both paths and the home, the update check the state path and the enabled flag.
- Ops' public surface is explicit re-exports (the command entry points, renderers and the types the root names, the environment, working-directory and reset-paths values among them, four config functions and `resolve_forest_multi`), and its implementation is private by Rust visibility.
- The root package holds only Root and Pure modules (the classification test). That constrains declarations, not composition semantics: the Root profile restricts the root's source effects, ops' implementation is private by Rust visibility, and command sequencing and the growth of ops' public surface follow the Checkable contract in `docs/architecture.md`.
- The forest crate's config schema still parses the update check's `version_check.enabled` key. A later change can move the schema and parse into the core and make parsing pure with `HOME` passed in, but who owns the tool-settings envelope (the update check's key beside the forest templates) is an open question for that change, not something the move settles.

Revisit if: a placement review finds a second responsibility, or a second composition root appears, such as an MCP server or another library consumer of the forest crates, another tool reading forests through the library, or the policy engine moving into a shared crate and needing a multi-crate fixture. A second host must also honour the update check's background handshake (the child is the host's own executable re-invoked with the internal argument, and the host rebuilds the same settings in it) or supply its own spawning boundary.
