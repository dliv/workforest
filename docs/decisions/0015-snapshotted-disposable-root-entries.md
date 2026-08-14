# 15. Snapshotted Disposable Forest-Root Entries

Date: 2026-07-28
Status: Accepted

## Context

A forest can accumulate incidental root entries outside its managed repository worktrees. IDE and agent tooling commonly create directories such as `.idea` and `.claude`. Ordinary `git forest rm` preserves unexpected residue, while `--force` recursively removes the whole forest and also relaxes unrelated Git safety checks.

A blanket rule for dot-prefixed entries is unsafe because files such as `.env` can contain unique data. Reading current template config during removal would also violate the self-contained metadata decision in ADR 0004: later config edits could silently expand an existing forest's deletion authority.

Nested configured paths introduce another boundary problem. A component between the forest root and the selected leaf can be a symlink, so ordinary path-based deletion cannot make the same simple no-follow guarantee as deletion of one top-level entry.

## Decision

Templates may declare repeatable, exact top-level `disposable_root_entries`. The resolved list is snapshotted into `.forest-meta.toml` when a forest is created. Metadata written before this feature defaults to an empty list.

`git forest rm --discard-root-entry <entry>` adds one-off deletion authority for an existing forest. The option is repeatable and cannot be combined with `--force`.

An entry must be one normal filename component. Absolute paths, nested paths, `.`, `..`, duplicates, `.forest-meta.toml`, and managed repository names are rejected. Duplicate and reserved-name comparisons conservatively fold case so deletion authority cannot alias metadata or repository names on default case-insensitive macOS/APFS volumes. Entries are not globs, and no values are built in by default.

Removal plans and structured results enumerate each authorized root action. Dry-run reports the same typed actions without changing the filesystem. Root-entry cleanup begins only after all Git cleanup checks and actions succeed, including under `--force`; each entry receives its observed outcome rather than inheriting an aggregate recursive-removal result. A failed root action or remaining unauthorized entry preserves forest metadata for recovery. Before the final `rmdir`, metadata is staged beside the forest and restored if directory removal fails, so an ordinary late entry or parent-permission failure cannot leave a residual forest undiscoverable.

Planning resolves each disposable entry to its actual on-disk spelling and rejects coexisting case aliases of metadata, repository, or disposable names. Force protects a case-equivalent repository spelling only when the configured repository path resolves to it; on a case-sensitive volume, a lone differently spelled entry is explicit force residue. This keeps dry-run and actual removal aligned on both case-sensitive and case-insensitive volumes. Reset uses the same metadata-staging boundary before recursively removing a forest, so a partial reset failure also leaves the forest discoverable for retry.

Mutation discovery treats inaccessible worktree bases or forest entries, including an offline target behind a configured base symlink, corrupt in-place metadata, and staged metadata left beside a worktree base as blocking recovery state. `reset` preserves config and state until that condition is repaired, and `rm --all` refuses to mutate a partially discoverable base. The observational `ls` command instead returns readable forests alongside structured per-directory findings for missing or unreadable metadata; it does not grant cleanup authority to those directories. A real forest directory may use the staged-file prefix without being mistaken for recovery state. JSON removal also rejects non-UTF-8 forest or root-entry paths during planning, before mutation, because its path fields cannot represent those names faithfully.

Before any repository or root mutation, removal requires the forest root itself to be a real directory. A symlinked or non-directory root is refused even under `--force`. For an authorized entry, a symlink or non-directory is unlinked; a real directory is removed with Rust's `std::fs::remove_dir_all`, which does not follow symbolic links on macOS and other supported Unix platforms.

The supported threat model is ordinary local, non-adversarial use on modern macOS/APFS. Hostile concurrent replacement of the entire forest root and mounted filesystems nested inside a disposable directory are out of scope. Closing those boundaries would require descriptor-relative capability traversal and a mount policy.

## Consequences

- Ordinary removal can clean explicitly authorized IDE or agent residue without weakening dirty-worktree, detached-head, unmerged-branch, or missing-source protections.
- Existing forests remain deterministic when template config changes.
- Agents and users can inspect exact deletion actions with `--dry-run --json`.
- Hidden entries retain protection unless explicitly configured or supplied for one removal; `.env` is not special-cased.
- Top-level-only authority is less expressive than an ignore-pattern file, but it is simpler to audit and avoids intermediate symlink traversal.
- Symlinked forest aliases that were previously discoverable are no longer removable through `git forest rm`; the alias must be inspected and unlinked manually.
- No filesystem or glob-matching dependency is added.
