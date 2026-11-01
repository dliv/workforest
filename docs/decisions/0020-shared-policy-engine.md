# 20. Shared policy engine with consumer-owned observation

Date: 2026-11-01
Status: Accepted locally; remote qualification pending

## Context

The second actual consumer requires shared metadata/package machinery while
retaining product-owned policy and native controls. The engine is a dev-dependency,
so normal-library policy code cannot import its types. Cargo spawning is already
inventoried at `crates/policy/src/metadata.rs`.

## Decision

Keep that subprocess adapter in the normal library and return raw JSON. Move
engine-typed policy and manifest observation into integration-test support.
Share metadata/manifest interpretation and dependency/feature evaluation through
arch-gates. Keep file/package classifications, allowed edges, external lists,
feature placement and direction reasons local. A single integration target
composes the existing 39 tests; no controls are dropped. Root effects,
external-reference and trait discovery also use the shared scanner; Root
permission names and the no-unsanctioned-traits policy remain local. Consolidate
production-token support in the engine and remove the duplicate normal-library
scanner after its controls have shared replacements. Add one local native
configuration test that preserves Workforest's method/print/debug baseline and
pairs permitted Root environment functions with actual method bans.

Parse explicit workspace membership, fail on malformed input, and prohibit
production dependencies on local policy and external engine packages across
normal/build/dev kinds, including aliases. Engine dependencies are dev-only.
Current pins use immutable local Git revisions while publication is deferred;
replace them with a reviewed published revision and qualify independently before
claiming portable builds or pushing consumers.

## Consequences

Runtime code/output is unchanged. The Cargo spawn stays under the same native
compiler inventory. Source scans and declared feature checks retain their
existing limits; they do not establish transitive purity or resolved capability
safety. Native configuration checks supplement the unchanged shell/compiler
inventory; they do not replace compiled proof. Reconsider if a new
consumer requires different observation facts or a different package policy;
add demonstrated inputs instead of a policy plugin framework.
