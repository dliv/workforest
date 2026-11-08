# Releases

Up: [Developer documentation](README.md) · [Development](development.md)

A release has two separate human decisions: merge the reviewed candidate, then
publish the checked merge and its tested artifacts. Guarded commands verify
identities and observed evidence; they do not authenticate a chat approval.
Record Dave's actual decisions and external review in task memory/conversation.
The repository currently permits other write paths; these guards prevent
accidental misuse, not an administrator deliberately bypassing the process.

## Prepare and review

Use Python 3.11+, Cargo, `just`, Node/npm and a current authenticated `gh` CLI.
The helpers use GitHub REST API version 2026-03-10, whose dispatch response
contains the exact run ID and URL. Start from a clean `release/...` branch with
current remote main included in it. Do not prepare on main or detached HEAD.

1. Select the next version with Dave; supported forms are `X.Y.Z` and
   `X.Y.Z-beta.N`. Run `just release-prepare VERSION`. This updates only the
   root Cargo version, lockfile root version, selected worker channel and current
   `RELEASE_NOTES.md`, then runs local checks and the worker dependency audit.
   Cargo must preserve the dependency graph. Unexpected resolution or check
   failure leaves edits for inspection; no reset, staging, commit, tag or push
   occurs. Notes for a new version start explicitly incomplete. Finish them.
2. Run the full local gate and obtain the separate product commit approval per
   [AGENTS](../AGENTS.md). Commit and push the release branch only when authorized.
   `release` and `release-beta` intentionally fail with guidance; they cannot
   publish. Never create an official tag to make a pre-tag check pass.
3. Arrange an external whole-release review with another agent/harness, such as
   Claude. Record reviewer identity, last successfully published channel release
   as baseline, exact reviewed commit C/tree, scope and finding dispositions.
   Include application code, dependencies, documentation, tests and release
   machinery. A feature-only review or the implementer's own review is insufficient.
   Any subsequent source change needs disposition and affected re-review.
4. Observe the named successful `CI` push run for that exact `release/...` head C.
   Both `just check` and `Audit worker deps` must succeed on the named current
   attempt. A PR's synthetic merge run may supplement this evidence, but does
   not replace exact-head branch CI. Record current remote main B and confirm
   B is included in C. If B/C moves, refresh affected review/CI and approval.

Request explicit **merge approval** naming B, C, the external review, CI run and
attempt, and the two-parent `--no-ff` method. Stop and wait for Dave's decision.
A passing check or a command argument is not that decision.

## Integrate and build the exact merge

After approval, from the clean reviewed candidate worktree:

```sh
just release-merge release/NAME B C CI_RUN CI_ATTEMPT
```

The helper checks exact remote refs and CI before creating M in a disposable
worktree rooted at B. It asserts ordered parents B,C and tree equality with C,
then pushes M to main without force. Existing occupied worktrees stay untouched;
update a primary main later only with a safe clean fast-forward. A concurrent
incompatible main push rejects integration. The local M remains reachable at
`refs/release-merges/M`; inspect it and the remote, then renew approval before
creating a replacement. It never silently rebases, squashes or recomputes M.

```sh
just release-check M
```

This validates M and dispatches `release-candidate.yml` on main with expected M.
It prints the exact created run ID/attempt/URL. The workflow refuses main/SHA
mismatch. With read-only repository permissions and no delivery secrets, it
validates pre-tag source and notes, runs the full Linux gate, audits worker deps,
and builds/packages both native macOS targets. Architecture, executable version,
archive contents and archived bytes are checked. No official tag/release exists
because of this operation. A failure in either build prevents the manifest job.

The manifest records repository, commit/tree, version/channel, run/attempt, both
worker channel versions, notes digest and the two archive filenames/targets,
artifact IDs and digests. The named artifacts include the attempt. Retention is
30 days; expiry requires a new complete candidate run and renewed publication
approval. Rerun **all jobs**, not only failed jobs: mixed-attempt proof is refused.
PR checks run the same native build steps for bootstrap proof but do not produce
a publication manifest. Actions artifacts in this public repository are public.

Inspect the exact run and its required jobs, download/verify its manifest and
artifacts, and present M, version/channel, run/attempt, notes and both SHA256s to
Dave for explicit **publication approval**. Stop and wait for that separate
decision. No rebuild or changed candidate may be substituted after approval.

## Publish and verify

After publication approval:

```sh
just release-publish VERSION M CANDIDATE_RUN CANDIDATE_ATTEMPT
```

This read-only preflight downloads the named artifacts, verifies the trusted
repository/workflow/event/branch/SHA and successful current attempt, checks exact
manifest identity, artifact provenance/expiry and both zip/file hashes, observes
worker/tap state and checks existing tag/release conflicts. It then dispatches
`release.yml` with those exact inputs. Dispatching publication is itself a
publication action; the local preflight does not replace approval.

Publication only runs from main, whose current SHA must still be M. Stable and
beta share a noncanceling concurrency group because they deploy one worker with
both channel versions. Before writes it repeats candidate and remote checks.
The selected channel may advance, and the other channel must retain its delivered
version. An obsolete candidate is refused. Equal selected versions are accepted
only with an existing matching tag for partial-publication recovery. API/worker
observation failures stop; manual actions outside this workflow are not serialized.

Ordered effects are:

- Create or verify the official tag at M; never force-move a tag.
- Create or verify the GitHub Release with approved notes and promote both exact
  downloaded archives. No Cargo build occurs. Existing assets must have identical
  bytes; missing assets may be uploaded, conflicting assets are never replaced.
- Deploy the worker from M when delivered channel versions differ, then read back
  both channel versions. A matching worker state is retained on retry.
- Fetch current tap main, update the selected formula's matching architecture
  URLs/digests, push normally and read back the result. Worker failure prevents
  tap mutation. Concurrent tap changes reject push instead of being overwritten.
- Recheck the release assets and report completed delivery.

Beta creates a prerelease and updates `git-forest-beta`; stable updates
`git-forest`. Both supported targets remain `aarch64-apple-darwin` and
`x86_64-apple-darwin`. Publication requires the existing Cloudflare credentials
and `HOMEBREW_TAP_TOKEN`; write permissions/secrets occur only in the effect job.
A workflow-created tag is not expected to trigger another workflow. Manual tag
pushes do not trigger publication.

## Failure and retry

GitHub, Cloudflare and Homebrew effects are not atomic. Read the completed-effect
messages and inspect remote tag, release, asset hashes, both worker versions and
tap formula before deciding what remains. A failed run does not roll anything back.
Rerun publication only with the **same approved M/version/candidate run/attempt**
while artifacts remain valid and main still equals M. Matching tag/assets and
worker state are retained; remaining work resumes. Renew approval for changed
inputs, replacement artifacts, another merge or another version. Expired artifacts
are never rebuilt during promotion. Never delete a conflicting asset or force a
tag to make a retry pass. If main moved, stop and prepare/review a current candidate.

## Proof and bootstrap limits

The release scripts' real-Git/Cargo fixtures and explicit API substitutes are in
[scripts/tests](../scripts/tests); `just release-tests` is part of `just check`.
They prove guard behavior, local merge topology/tree identity, byte promotion and
fault ordering. They do not prove hosted Actions, native runner availability,
Cloudflare deployment or delivery credentials.

A new dispatchable workflow must first land on the default branch. Before that
approved tooling merge, use local checks and authorized PR native builds. After
it, run the actual candidate workflow on that exact tooling M without publishing.
The current v0.8.1 source/notes can seed that proof; an old version's existing tag
at another commit intentionally prevents publication. No new version is selected
by the bootstrap. Hosted candidate proof remains outstanding until observed, and
production publication remains outstanding until a separately approved release
actually uses this path.
