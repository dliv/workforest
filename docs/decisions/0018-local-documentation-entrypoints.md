# 18. Local Documentation Entrypoints

Date: 2026-10-21
Status: Accepted

## Context

The crate split made responsibilities explicit in code and policy, but a reader
landing in a feature or infrastructure directory had to reconstruct its map
from source comments and the central architecture document. Contributor
instructions also mixed navigation with engineering detail.

## Decision

- Keep the repository README as the human-facing product and quick-start page.
  `AGENTS.md` holds contributor rules and a when-to-read document router.
- Give each feature and major organizational area a local README. Cover a
  feature's roles together: the forest README maps both core and ops. Do not
  create another README in every directory automatically.
- A feature entrypoint states its responsibility and consumer, owned outcome
  and guarantees, why the nearest feature is not its owner, its modules and
  kinds, dependencies, proofs and upcoming work.
- Each fact has one owning document. Other documents link to it; ADRs may
  restate facts needed to explain their decisions. Reference lives beside
  the area it describes, and changed relative links are checked.
- [Design principles](../design-principles.md) owns the six feature-start
  questions. Routine work may reference an existing example and its checks;
  these answers inform the design contract, not a new mandatory file format.

## Consequences

Readers can enter through the directory they are changing and find its proof
and rule owners. New areas need an entrypoint and a router link. Source comments
still explain APIs and local algorithms; READMEs do not duplicate them. Current
implementation and agreed future work are labeled separately.

These are documentation and review practices. No new source-code gate or
runtime behavior is introduced. The root's commit and release instructions
remain in `AGENTS.md`.

Revisit if: facts drift between entrypoints, a reader cannot find the owning
document from the router, or a README duplicates source detail without helping
placement or validation.
