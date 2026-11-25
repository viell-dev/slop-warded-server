#!/usr/bin/env bash
# Check the library. scripts/check-templates.sh checks the templates.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(cargo pkgid --locked --package warded-server)"
version="${version##*[@#]}"
version="${version%$'\r'}"

# Everything that names the release to install names this one.
for file in README.md templates/*/Cargo.toml.liquid; do
    others="$(grep --extended-regexp --only-matching \
        '(tag = "|--tag )[0-9][^"` ]*' "$file" | grep --invert-match --extended-regexp \
        "(tag = \"|--tag )${version//./\\.}$" || true)"
    if [[ -n "$others" ]] || ! grep --quiet --fixed-strings "$version" "$file"; then
        echo "$file does not consistently name $version" >&2
        exit 1
    fi
done
grep --quiet --extended-regexp "^## ${version//./\\.} - " CHANGELOG.md ||
    { echo "CHANGELOG.md has no section for $version" >&2; exit 1; }

cargo fmt -- --check
rustfmt --edition 2024 --check templates/*/src/main.rs
cargo clippy --all-targets --locked -- -D warnings
for features in "" http3 self-signed warded http3,warded self-signed,warded; do
    cargo clippy --no-default-features --features "$features" --all-targets --locked -- -D warnings
done
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
