setup:
    git config core.hooksPath .githooks

check: fmt-check lint test policy-control

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --locked --no-default-features --features stable --all-targets -- -D warnings
    cargo clippy --workspace --locked --no-default-features --features beta --all-targets -- -D warnings

policy-control:
    bash scripts/policy-control.sh

build:
    cargo build

test:
    cargo test --workspace --locked --no-default-features --features stable
    cargo test --workspace --locked --no-default-features --features beta

test-linux:
    docker run --rm -v "{{justfile_directory()}}:/work" -w /work rust:latest cargo test

loc:
    tokei src tests

# macOS-only (sed -i '' syntax). Bumps version, commits, tags, pushes, deploys worker.
release version:
    #!/usr/bin/env bash
    set -euo pipefail

    # 1. Verify clean state and passing checks
    just check
    just test
    (cd worker && npm ci --silent && npm audit)

    # 2. Update version in Cargo.toml and wrangler.toml
    sed -i '' 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    sed -i '' 's/^LATEST_VERSION_STABLE = ".*"/LATEST_VERSION_STABLE = "{{version}}"/' worker/wrangler.toml

    # 3. Rebuild to update Cargo.lock
    cargo check
    just check

    # 4. Commit, tag, push (push tag by name per CLAUDE.md)
    git add Cargo.toml Cargo.lock worker/wrangler.toml
    git commit -m "chore: bump version to {{version}}"
    git tag "v{{version}}"
    git push
    git push origin "v{{version}}"

    echo "Released v{{version}} — worker deploys via CI"

# macOS-only. Bumps beta version, commits, tags, pushes.
release-beta version:
    #!/usr/bin/env bash
    set -euo pipefail

    # 1. Verify clean state and passing checks
    just check
    just test
    (cd worker && npm ci --silent && npm audit)

    # 2. Update version in Cargo.toml and wrangler.toml
    sed -i '' 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    sed -i '' 's/^LATEST_VERSION_BETA = ".*"/LATEST_VERSION_BETA = "{{version}}"/' worker/wrangler.toml

    # 3. Rebuild to update Cargo.lock
    cargo check
    just check

    # 4. Commit, tag, push (push tag by name per CLAUDE.md)
    git add Cargo.toml Cargo.lock worker/wrangler.toml
    git commit -m "chore: bump beta version to {{version}}"
    git tag "v{{version}}"
    git push
    git push origin "v{{version}}"

    echo "Released v{{version}} (beta) — worker deploys via CI"

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
