#!/usr/bin/env bash
# Check the library. scripts/check-templates.sh checks the templates.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(cargo pkgid --locked --package warded-server)"
version="${version##*[@#]}"
version="${version%$'\r'}"

# Everything that names the release to install names this one.
for file in README.md templates/*/Cargo.toml.liquid docs/*.md; do
    others="$(grep --extended-regexp --only-matching \
        '(tag = "|--tag )[0-9][^"` ]*' "$file" | grep --invert-match --extended-regexp \
        "(tag = \"|--tag )${version//./\\.}$" || true)"
    if [[ -n "$others" ]]; then
        echo "$file names another release than $version: $others" >&2
        exit 1
    fi
done
for file in README.md templates/*/Cargo.toml.liquid docs/getting-started.md; do
    grep --quiet --fixed-strings "$version" "$file" ||
        { echo "$file does not name $version" >&2; exit 1; }
done
grep --quiet --extended-regexp "^## ${version//./\\.} - " CHANGELOG.md ||
    { echo "CHANGELOG.md has no section for $version" >&2; exit 1; }

cargo fmt --all -- --check
rustfmt --edition 2024 --check templates/*/src/main.rs
# The workspace includes the documentation examples' crate, whose page checks are linted too.
cargo clippy --workspace --all-targets --locked -- -D warnings
for features in "" http3 self-signed warded http3,self-signed http3,warded self-signed,warded; do
    cargo clippy --no-default-features --features "$features" --all-targets --locked -- -D warnings
done
cargo test --locked
# The documentation site's Rust examples, and the check that every page's are run.
cargo test --package warded-server-docs --locked
cargo test --no-default-features --doc --locked
export RUSTDOCFLAGS="-D warnings"
cargo doc --no-deps --locked
cargo doc --no-default-features --no-deps --locked
