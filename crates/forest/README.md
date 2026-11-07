# Forest

Up: [Library crates](../README.md) · [Architecture](../../docs/architecture.md)

The forest feature creates, tends, inspects and removes a group of worktrees
for a user or an agent. It owns the forest directory, its metadata and the
worktrees and branches its operations manage. Its guarantees include explicit
plans, typed outcomes, removal safeguards, and enough saved metadata to operate
on an existing forest without consulting its template again. Their exact
contracts live in [ADRs 0003, 0004 and 0015](../../docs/decisions/ADR_INDEX.md).

There is no other domain feature to own this work. `init` and templates extend
how forests are created; git is a technical client, settings configure the
feature, and the update check serves a different protocol concern. The
[placement record](../../docs/candidate-features.md) owns those verdicts and
their reconsideration triggers.

## Modules and boundaries

| Crate | Kind and responsibility | Entry points |
| --- | --- | --- |
| `git-forest-core` in [core](core/src/lib.rs) | Pure: validated paths, metadata types, config format, command decisions, results and renderers | [config](core/src/config.rs), [paths](core/src/paths.rs), [plans/results](core/src/plan/mod.rs) |
| `git-forest-ops` in [ops](ops/src/lib.rs) | Effectful: observe and execute commands, run git, discover forests, read/write metadata and configuration | [commands](ops/src/commands/mod.rs), [config I/O](ops/src/config.rs), [forest resolution](ops/src/forest.rs) |

The [composition root](../../src/README.md) supplies environment and working
directory values, resolved file paths and time. Commands return data rather
than print. Core depends only on its allowed libraries; ops depends on core
and its own allowed libraries, never update-check or another feature. The
[architecture rules](../../docs/architecture.md) own the exact allowlists,
visibility contract and the permanent `reset -> rm` helper-sharing boundary.

Read-only lock reporting uses [lock observations](ops/src/lock_state.rs) in ops
and [lock state and rendering](core/src/plan/lock_state.rs) in core. `ls` reuses
one inventory per canonical source path within its invocation, including failed
observations; each worktree is matched separately. `status` reads a fresh
inventory per repo. The [user-facing contract](../../README.md#commands) describes
the output and unavailable-state semantics.

Removal observes repo safety before mutation and core decides explicit Ready or
Blocked readiness independently of root residue. The rm-private `PreparedRemoval`
binds its plan, force policy, observation and decision; preview uses that exact
decision and adds the root-cleanup forecast afterward. Execution accepts only the
prepared value and keeps live refusal checks. Its content observations override
Git's status suppression settings and inspect worktree-specific submodule
metadata plus indexed gitlinks, independently of active/ignore configuration.
Failed observations block ordinary removal; force retains its existing override. Batch removal refreshes each forest
before handling it, after strict discovery. Reset keeps its distinct executor and
legacy preview policy. The [user contract](../../README.md#rm) describes scope,
force overrides and the remaining race/partial-cleanup limits.

## Proofs

- Core unit tests beside the decisions, newtypes and config format assert
  pure behavior, with rendering snapshots under `core/src/plan/`.
- Ops unit tests beside each workflow use real repositories built by
  [testutil](ops/src/testutil.rs); they assert filesystem, worktree and branch
  effects as well as result data.
- [second_root.rs](ops/tests/second_root.rs) drives the public ops API with
  explicit environment, home, working directory and time values. It does not
  cover most init/reset paths. The separate
  [update-check host proof](../update-check/README.md#proofs-and-generated-contract)
  exercises edge resources and a real test-owned parent/child pair.
- [Process tests](../../tests/README.md) exercise the built CLI's output,
  exits, dry-runs and partial failures. [Policy checks](../policy/README.md)
  protect the package and purity boundaries.
- `ls_and_status_report_worktree_locks` exercises real locks, missing records,
  unavailable sources and terminal escaping. `ls_reuses_source_lock_inventories_per_invocation`
  counts real Git subprocesses across shared sources, checks separate forest
  locks, source aliases on Unix, failed inventories and fresh later invocations.

## What comes next

No second feature or extra abstraction is assumed. Use the placement record
when new behavior arrives and preserve the public ops seam when another host
needs it. Update-check resource and process ownership belongs to the
[edge and composition root](../../docs/decisions/0019-app-owned-update-check-resources.md),
not to a new forest subfeature.
