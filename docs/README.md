# Developer documentation

Up: [Repository README](../README.md)

Start with the document for the work you are doing. [AGENTS.md](../AGENTS.md)
holds contributor instructions and the when-to-read router.

| Document | Owns |
| --- | --- |
| [Development](development.md) | Local commands, validation and release mechanics |
| [Design principles](design-principles.md) | Judgment rules and the six feature-start questions |
| [Architecture](architecture.md) | Module/crate rules, enforcement, exceptions and proof limits |
| [Candidate features](candidate-features.md) | The feature placement test and dated placement decisions |
| [Decision records](decisions/ADR_INDEX.md) | Why the current design was chosen and when to reconsider it |
| [Agent usage](agent-instructions.md) | Instructions embedded in the product for agents using the CLI |

The local code entrypoints are [crates](../crates/README.md), the
[composition root](../src/README.md), [process tests](../tests/README.md),
[scripts](../scripts/README.md) and the [update worker](../worker/README.md).
The [forest feature](../crates/forest/README.md) covers both its core and ops
crates; each role directory does not need another copy of that map.

Each fact has one owning document. Link to its owner rather than copying its
details; ADRs may repeat facts needed to explain a decision. Local READMEs
describe current responsibilities, dependencies and proofs. Mark future work
as future work. Check changed relative links when editing documentation.
This convention is recorded in [ADR 0018](decisions/0018-local-documentation-entrypoints.md).
