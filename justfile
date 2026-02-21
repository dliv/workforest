setup:
    git config core.hooksPath .githooks

check:
    cargo fmt --all -- --check
    cargo clippy --all-targets

build:
    cargo build

test:
    cargo test

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

    # 2. Update version in Cargo.toml and wrangler.toml
    sed -i '' 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    sed -i '' 's/^LATEST_VERSION = ".*"/LATEST_VERSION = "{{version}}"/' worker/wrangler.toml

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

# --- Worker (Cloudflare) ---

generate-types:
    TS_RS_EXPORT_DIR=. cargo test export_bindings -- --ignored

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
