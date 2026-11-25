#!/usr/bin/env bash
# Generate a project from every template, with the library from this checkout, and check it.
# Needs cargo-generate.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository="$(pwd -W 2>/dev/null || pwd)"
work="$repository/target/templates"

rm -rf "$work/projects"
mkdir -p "$work/projects"

for template in templates/*/; do
    name="$(basename "$template")"
    project="$work/projects/my-$name"
    echo "== $name"
    cargo generate --path "$repository" "templates/$name" --name "my-$name" \
        --destination "$work/projects" --vcs none --silent
    if grep --recursive --files-with-matches 'project-name' "$project"; then
        echo "the files above still hold a placeholder" >&2
        exit 1
    fi
    # The release that the template names may not exist yet.
    sed -i "s#^warded-server = .*#warded-server = { path = \"$repository\" }#" "$project/Cargo.toml"
    printf '\n[workspace]\n' >>"$project/Cargo.toml"
    (
        cd "$project"
        export CARGO_TARGET_DIR="$work/target"
        cargo fmt -- --check
        cargo clippy --all-targets -- -D warnings
        cargo test
    )
done
