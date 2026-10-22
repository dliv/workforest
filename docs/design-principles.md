# Design principles

Up: [Developer documentation](README.md)

These are judgment rules. [Architecture](architecture.md) owns the gated and
checkable rules; [ADRs](decisions/ADR_INDEX.md) own the reasons for the current
design. Use the [feature placement test](candidate-features.md) before making
a new feature, and the [forest README](../crates/forest/README.md) when work
extends the existing one.

## Engineering practice

- Make illegal states unrepresentable. Prefer validated newtypes such as
  `AbsolutePath`, `RepoName`, `ForestName` and `BranchName` over repeated runtime
  validation. Validate once at construction and trust the invariant internally.
- Do not add `bail!` or `debug_assert!` for something a type already guarantees.
  `debug_assert!` is for invariants types cannot express, such as collection
  uniqueness or a precondition at an untyped `&Path` boundary
  ([ADR 0010](decisions/0010-debug-assertions-for-invariants.md)).
- Keep decisions pure and effects explicit. Forest command operations own
  observation and execution; core owns decisions and result data; the root
  parses inputs and renders outcomes. Follow the existing
  [composition contract](architecture.md#root-composition).
- Keep the happy path readable and use enums and exhaustive matches for
  meaningful states and outcomes. Partial success must remain distinguishable
  from completion; terminal diagnostics need no elaborate recovery mechanism.
- Name the invariant a boundary protects. Prefer concrete calls; the
  [trait rule](architecture.md#no-production-traits) requires a second real
  production implementation, not a test double.
- Preserve useful debug handles: typed plans, deterministic output, JSON
  contracts and dry-run proofs. Test effects against real git repositories
  ([ADR 0007](decisions/0007-e2e-tests-with-real-git-repos.md)).

## Feature-start questions

Before adding behavior or changing a boundary, record six short answers in
the design artifact for that work:

1. **Behavior:** which observable behavior and refusal cases change?
2. **Placement and invariants:** does this extend the forest responsibility or
   justify a new feature; which modules own it and what must remain true?
3. **Trust and effects:** what enters untrusted, who supplies trusted facts,
   and who owns state changes or external actions?
4. **Failure and lifecycle:** what happens on duplicates, concurrency,
   cancellation or partial failure; which cases are irrelevant?
5. **Proof:** what do types, gates and tests each establish; what concrete
   check will prove the assembled behavior?
6. **Architecture change:** does this follow an existing example or change a
   boundary or policy?

Routine work can reference an established example and its checks. These
answers guide the design contract in [ADR 0008](decisions/0008-contract-driven-development.md);
they do not replace it or require a separate document for every small edit.
They are reviewed, not enforced by a new source-code gate.
