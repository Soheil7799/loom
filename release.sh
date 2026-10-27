#!/usr/bin/env bash
# Gets a release ready: `./release.sh 0.2.0` sets that version (Cargo.toml, Cargo.lock),
# commits it and tags v0.2.0. Pushing the tag (`git push origin main v0.2.0`) has GitHub
# build and publish the release (.github/workflows/release.yml).
set -euo pipefail
cd "$(dirname "$0")"

version=${1:?usage: ./release.sh <version>, such as 0.2.0}
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Not a version: $version (one such as 0.2.0)" >&2
    exit 1
fi
if [[ -n $(git status --porcelain) ]]; then
    echo "Commit or stash your changes first." >&2
    exit 1
fi
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
    echo "v$version is tagged already." >&2
    exit 1
fi

sed -i "0,/^version = .*/s//version = \"$version\"/" Cargo.toml
cargo update --workspace --offline --quiet # Cargo.lock follows
if [[ -n $(git status --porcelain) ]]; then
    git commit -q -am "loom $version"
fi
git tag -a "v$version" -m "loom $version"
echo "Tagged v$version. To publish it: git push origin main v$version"
