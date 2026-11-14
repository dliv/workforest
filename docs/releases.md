# Releases

Up: [Developer documentation](README.md) · [Development](development.md)

The maintainer approves the merge of the reviewed candidate and separately
approves publication of the tested artifacts. The commands verify the commit,
CI, and artifact identities. See [AGENTS.md](../AGENTS.md#working-with-humans)
for approval of public repository actions.

## Prepare and review

Use Python 3.11+, Cargo, `just`, Node/npm and a current authenticated `gh` CLI.
Start from a clean `release/...` branch that includes current remote `main`.
Preparation requires a clean, non-main branch. The merge command also enforces
the `release/...` name and current-main ancestry.

A maintenance-only merge can keep the current version and its complete notes.
In that case, start at step 2.

1. Agree on the version with the maintainer. Supported forms are `X.Y.Z` and
   `X.Y.Z-beta.N`. Run `just release-prepare VERSION`. This updates only the
   root Cargo version, lockfile root version, selected worker channel and current
   `RELEASE_NOTES.md`, then runs local checks and the worker dependency audit.
   Cargo must preserve the dependency graph. If a check fails, inspect the local
   edits. Finish the notes for a new version before submitting it for review.
2. Run the full local gate and obtain approval for the local commit. The old
   `release` and `release-beta` recipes direct you to this procedure. Create
   the official tag during publication.
3. Obtain an independent external agent review of the whole release. Record the
   reviewer, last successfully published release for the channel, exact reviewed
   commit C and tree, scope, and how each finding was resolved.
   Include application code, dependencies, documentation, tests and release
   tooling. Review any subsequent source changes within their affected scope.
4. Obtain approval to push the reviewed branch head C. Verify its exact `CI`
   push run. Both `just check` and `Audit worker deps` must succeed on the
   named current attempt. A PR run supplies additional evidence for its
   synthetic merge.
   Record current remote `main` B and confirm B is included in C. If B or C
   moves, refresh the affected review and CI evidence before seeking approval.

Request explicit **merge approval** from the maintainer. Include B, C, the
external review, the CI run and attempt, and the two-parent `--no-ff` method.

## Integrate and build the exact merge

After approval, from the clean reviewed candidate worktree:

```sh
just release-merge release/NAME B C CI_RUN CI_ATTEMPT
```

`release-merge` checks exact remote refs and CI, creates M in a disposable
worktree rooted at B, verifies ordered parents B and C and tree equality with C,
then pushes M to `main` without force. It respects existing worktree checkouts.
If a concurrent update rejects the push, inspect the remote and the preserved
local `refs/release-merges/M` ref. Obtain approval for a replacement merge using
the updated evidence.

Obtain separate approval to dispatch the candidate build for M:

```sh
just release-check M
```

`release-check` validates M and dispatches `release-candidate.yml` on `main`
using M as the expected SHA. It prints the run ID, attempt, and URL. The workflow
file must exist on the default branch for dispatch. The run verifies its source
and notes, runs the full Linux gate, audits worker dependencies, and packages
both native macOS targets with architecture, version, and archive checks. It
uses read-only repository permissions. The publication workflow creates tags
and releases.

The manifest records repository, commit/tree, version/channel, run/attempt, both
worker channel versions, notes digest and the two archive filenames/targets,
artifact IDs and digests. The named artifacts include the attempt. Retention is
30 days. After expiry, run a complete candidate build and renew publication
approval. Select **rerun all jobs** so all evidence comes from a single attempt.
PR checks run the same native build steps. Publication requires a manifest from
an exact-main dispatch. Signed-in GitHub users with read access can download
the artifacts.

Inspect the exact run and its required jobs, then download and verify the
manifest and artifacts. This completes verification for a maintenance-only
merge. For a release, request explicit **publication approval** from the
maintainer with M, version/channel, run/attempt, notes, and both archive
SHA-256 digests. That approval covers those exact inputs and artifacts.

## Publish and verify

After publication approval:

```sh
just release-publish VERSION M CANDIDATE_RUN CANDIDATE_ATTEMPT
```

`release-publish` downloads the named artifacts during preflight and verifies
the repository, workflow, event, branch, SHA, and successful current attempt.
It checks the manifest, artifact origin and expiry, and ZIP and file hashes.
It reads worker and tap state, checks for tag and release conflicts, then
dispatches `release.yml` with those exact inputs.

Publication runs from `main`, whose current SHA must still be M. Stable and
beta share a concurrency group because both deploy the same worker. An active
publication keeps running when another is dispatched. A later dispatch can
replace a pending run, so inspect the exact run's status before continuing.
[GitHub concurrency behavior](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency)

Before writes the workflow repeats candidate and remote checks.
The selected channel may advance, and the other channel must retain its delivered
version. An obsolete candidate is refused. Equal selected versions are accepted
only with an existing matching tag for partial-publication recovery. API/worker
observation failures stop publication.

Ordered effects are:

- Create or verify the official tag at M; never force-move a tag.
- Create or verify the GitHub Release with approved notes and promote both exact
  downloaded archives. No Cargo build occurs. Existing assets must have identical
  bytes; missing assets may be uploaded, conflicting assets are never replaced.
- Recheck both delivered channel versions before deploying the worker from M.
  Retain a matching worker on retry. Confirm both versions before touching the tap.
- Fetch tap `main`, advance the selected formula, and verify its architecture
  URLs and digests after pushing. For an existing same-version formula, both
  URLs and digests must already match. A concurrent tap update can reject the push.
- Recheck the release assets and both worker versions before reporting delivery.

Beta creates a prerelease and updates `git-forest-beta`; stable updates
`git-forest`. Both supported targets remain `aarch64-apple-darwin` and
`x86_64-apple-darwin`. Publication requires the existing Cloudflare credentials
and `HOMEBREW_TAP_TOKEN`. The publication job holds these credentials and
repository write permissions.

## Failure and retry

GitHub, Cloudflare and Homebrew effects are not atomic. Read the completed-effect
messages and inspect remote tag, release, asset hashes, both worker versions and
tap formula before deciding what remains. A failed run does not roll anything back.
Before rerunning publication, obtain approval for another dispatch with the
**same M/version/candidate run/attempt**. Artifacts must still be valid and
`main` must still equal M. Matching tag, assets, and worker state are retained
as the remaining work resumes. Obtain new approval for changed inputs,
replacement artifacts, another merge or another version. Expired artifacts
are never rebuilt during promotion. Never delete a conflicting asset or force a
tag to make a retry pass. If main moved, stop and prepare/review a current candidate.

For local fixture coverage and its limits, see
[Release operations](../scripts/README.md#release-operations).
