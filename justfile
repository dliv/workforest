setup:
    git config core.hooksPath .githooks

check: fmt-check lint test release-tests policy-control

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --locked --no-default-features --features stable --all-targets -- -D warnings
    cargo clippy --workspace --locked --no-default-features --features beta --all-targets -- -D warnings

release-tests:
    PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -p 'test_*.py'

policy-control:
    bash scripts/policy-control.sh

build:
    cargo build

# Runs with the global and system git config disabled: the fixtures build real repositories, and a
# developer's commit signing or global hooks would otherwise fail their commits (docs/architecture.md, "Tests").
test:
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 cargo test --workspace --locked --no-default-features --features stable
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 cargo test --workspace --locked --no-default-features --features beta

test-linux:
    docker run --rm -v "{{justfile_directory()}}:/work" -w /work rust:latest cargo test --workspace

loc:
    tokei src crates tests

# quote() passes each argument as one shell-escaped value.
release version:
    @echo 'Combined publication is retired. Follow docs/releases.md: prepare, review, merge, check, publish.' >&2
    @exit 1

release-beta version:
    @echo 'Combined publication is retired. Follow docs/releases.md.' >&2
    @exit 1

release-prepare version:
    python3 scripts/release.py prepare {{quote(version)}}

release-merge branch expected_main reviewed_head ci_run ci_attempt:
    python3 scripts/release.py merge {{quote(branch)}} {{quote(expected_main)}} {{quote(reviewed_head)}} {{quote(ci_run)}} {{quote(ci_attempt)}}

release-check merge_sha:
    python3 scripts/release.py check {{quote(merge_sha)}}

release-publish version merge_sha candidate_run candidate_attempt:
    python3 scripts/release.py publish {{quote(version)}} {{quote(merge_sha)}} {{quote(candidate_run)}} {{quote(candidate_attempt)}}

# --- Worker (Cloudflare) ---

generate-types:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target
    cargo test -p git-forest-update-check --lib export_bindings -- --ignored --color never 2>&1 | tee target/generate-types.log
    grep -Eq '^test result: ok\. [1-9][0-9]* passed' target/generate-types.log || { echo "generate-types: no export test ran; check the package and test names" >&2; exit 1; }

worker-deploy:
    cd worker && npx wrangler deploy

worker-db-create:
    cd worker && npx wrangler d1 create git-forest-version-check

worker-db-migrate:
    cd worker && npx wrangler d1 execute git-forest-version-check --remote --file=schema.sql

worker-logs:
    cd worker && npx wrangler tail

worker-query sql:
    cd worker && npx wrangler d1 execute git-forest-version-check --remote --command "{{sql}}"
