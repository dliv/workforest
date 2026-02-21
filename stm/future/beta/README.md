# Beta Releases via Homebrew

## Approach: separate `git-forest-beta` formula

Add a second formula to the Homebrew tap (`dliv/homebrew-tap` or similar):

- `git-forest` — stable releases, tagged `v0.x.y`
- `git-forest-beta` — pre-release builds, tagged `v0.x.y-beta.N`

Users opt in explicitly: `brew install dliv/tap/git-forest-beta`. The two formulae install to different binary names or the beta conflicts/replaces stable — TBD.

## Why not the alternatives

- **`--HEAD`**: Builds from branch tip, no versioning. Users can't pin to a specific beta, and there's no way to tell them "try beta 2, we fixed X." Also no prebuilt bottles.
- **Pre-release tags on the main formula**: Awkward to toggle. Homebrew version comparison treats `-beta` as older than the release, so `brew upgrade` behaves unexpectedly.

## Open questions

- **Binary name conflict**: Should `git-forest-beta` install as `git-forest` (with `conflicts_with "git-forest"`) or as a separate `git-forest-beta` binary? Conflicting is simpler for users who want to swap.
- **Automation**: The `just release` recipe would need a `just release-beta` variant that tags `v0.x.y-beta.N` and updates the beta formula.
- **Bottles**: GitHub Actions can build bottles for the beta formula the same way as stable.

## Priority

Low. No immediate need — useful once there are enough users that breaking changes need a soak period.
