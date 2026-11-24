#!/usr/bin/env bash
# Runs the release workflow's build and tests (.github/workflows/release.yml) as GitHub will:
# in an Ubuntu 24.04 container, with only the packages the workflow installs. A release is
# tagged only once this passes (release.sh runs it), so a pushed tag's build doesn't fail.
#
# Needs podman. The cargo and rustup caches are kept in podman volumes and the build in
# target/ci, so only the first run builds everything. CI_JOBS sets the build jobs (default 4:
# a whole build from scratch is long and heavy).
set -euo pipefail
cd "$(dirname "$0")"

workflow=.github/workflows/release.yml
packages=$(sed -n 's/.*apt-get install -y --no-install-recommends //p' "$workflow")
if [[ -z $packages ]]; then
    echo "No apt-get line found in $workflow: ci-check.sh needs updating." >&2
    exit 1
fi

podman run --rm \
    -v "$PWD":/src:Z \
    -v loom-ci-cargo:/root/.cargo \
    -v loom-ci-rustup:/root/.rustup \
    -e CARGO_TARGET_DIR=/src/target/ci \
    -e CARGO_BUILD_JOBS="${CI_JOBS:-4}" \
    -w /src \
    docker.io/library/ubuntu:24.04 \
    bash -euo pipefail -c "
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends ca-certificates curl unzip git build-essential pkg-config $packages >/dev/null
        [ -x /root/.cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
        export PATH=/root/.cargo/bin:/root/.bun/bin:\$PATH
        rustup update stable --no-self-update
        [ -x /root/.bun/bin/bun ] || curl -fsSL https://bun.sh/install | bash >/dev/null
        (cd web && bun install --frozen-lockfile && bun run build)
        cargo build --locked --workspace
        cargo test --locked --workspace
        cargo build --release --locked --workspace
        echo 'ci-check: passed'
    "

# Files the build writes that are kept in git (the web page's API types, from the API spec):
# changed by the build means the committed copy is stale.
if ! git diff --quiet -- web/src/lib/api-types.ts; then
    echo "ci-check: web/src/lib/api-types.ts is out of date (the build regenerated it): commit it." >&2
    exit 1
fi
