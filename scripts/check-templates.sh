#!/usr/bin/env bash
# Generate a project from every template, with the library from this checkout, and check it: its
# formatting, lints and tests, and what it serves when `cargo run` starts it, as a user would.
# Needs cargo-generate and curl.
#
# With --browser, also use the website template's pages in Chrome, which needs Google Chrome,
# Node.js and `npm ci` in scripts/browser.
set -euo pipefail

browser=false
case "${1-}" in
    --browser) browser=true ;;
    "") ;;
    *)
        echo "usage: $0 [--browser]" >&2
        exit 2
        ;;
esac

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository="$(pwd -W 2>/dev/null || pwd)"
work="$repository/target/templates"
export CARGO_TARGET_DIR="$work/target"

# The development address that the templates' .cargo/config.toml sets.
development="http://127.0.0.1:3000/"
# How long a project may take to build and to begin serving, in seconds.
start_limit=300
# How long it may take to stop.
stop_limit=15

rm -rf "$work/projects"
mkdir -p "$work/projects"

server=""
log=""

stop_server() {
    [[ -n "$server" ]] || return 0
    local pid="$server"
    server=""
    kill "$pid" 2>/dev/null || true
    for ((waited = 0; waited < stop_limit * 10; waited++)); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        fail "the server did not stop within $stop_limit seconds"
    fi
    wait "$pid" 2>/dev/null || true
}
trap stop_server EXIT

fail() {
    echo "check-templates: $name: $*" >&2
    if [[ -n "$log" && -f "$log" ]]; then
        echo "--- the end of $log:" >&2
        tail -n 40 "$log" >&2
    fi
    exit 1
}

# Start the project with `cargo run`, and wait until it logs the address to open.
start_server() {
    log="$work/$name.log"
    # Unset, so that .cargo/config.toml applies. HTTPS takes a free port, and the log no colours.
    (cd "$project" && exec env -u WARDED_DEV_ADDR NO_COLOR=1 WARDED_HTTPS_ADDR=127.0.0.1:0 \
        cargo run --quiet) >"$log" 2>&1 &
    server=$!
    for ((waited = 0; waited < start_limit; waited++)); do
        if grep --quiet 'open http' "$log"; then
            return 0
        fi
        kill -0 "$server" 2>/dev/null || fail "the server ended before it logged an address"
        sleep 1
    done
    fail "the server logged no address to open within $start_limit seconds"
}

# What a GET of the URL answers, failing on an error status.
fetch() {
    curl --fail --silent --show-error --max-time 10 "$@" ||
        fail "curl $* failed"
}

# Run the project as a user would, and check what it serves.
check_serving() {
    start_server
    local open https
    open="$(grep --only-matching 'open http[^ ]*' "$log" | head -n 1)"
    [[ "${open#open }" == "$development" ]] ||
        fail "it says to ${open:-open nothing}, and not $development from .cargo/config.toml"
    https="$(grep --only-matching ' https=[0-9.:]*' "$log" | head -n 1)"
    https="https://${https# https=}/"
    [[ "$https" =~ ^https://127\.0\.0\.1:[0-9]+/$ ]] || fail "the log names no HTTPS address"

    local page secure
    page="$(fetch "$development")"
    secure="$(fetch --insecure "$https")"
    [[ "$page" == *"$expected"* ]] || fail "$development does not answer with $expected: $page"
    [[ "$secure" == *"$expected"* ]] || fail "$https does not answer with $expected: $secure"
    [[ "$secure" != *_tower-livereload* ]] || fail "$https has the reload script"
    if [[ "$name" == website ]]; then
        [[ "$page" == *_tower-livereload/event-stream* ]] ||
            fail "$development has no reload script"
        # The stream stays open, so curl ends it.
        local events
        events="$(curl --silent --max-time 2 "${development}_tower-livereload/event-stream" || true)"
        [[ "$events" == "event: init"* ]] || fail "the reload stream sent: $events"
    else
        # It is not a page, so it stays as it is.
        [[ "$page" != *_tower-livereload* ]] || fail "$development added the script to $expected"
    fi
    stop_server
}

for template in templates/*/; do
    name="$(basename "$template")"
    project="$work/projects/my-$name"
    log=""
    echo "== $name"
    cargo generate --path "$repository" "templates/$name" --name "my-$name" \
        --destination "$work/projects" --vcs none --silent --no-workspace
    if grep --recursive --files-with-matches 'project-name' "$project"; then
        fail "the files above still hold a placeholder"
    fi
    [[ -f "$project/.cargo/config.toml" ]] || fail ".cargo/config.toml was not generated"
    # The release that the template names may not exist yet.
    sed -i "s#^warded-server = .*#warded-server = { path = \"$repository\" }#" "$project/Cargo.toml"
    printf '\n[workspace]\n' >>"$project/Cargo.toml"
    (
        cd "$project"
        cargo fmt -- --check
        cargo clippy --all-targets -- -D warnings
        cargo test
    )
    case "$name" in
        minimal) expected="Hello" ;;
        api) expected='"endpoints"' ;;
        website) expected="<h1>Welcome</h1>" ;;
        *) fail "the check does not know what this template serves" ;;
    esac
    check_serving
    if [[ "$name" == website && "$browser" == true ]]; then
        echo "== $name in a browser"
        WEBSITE_PROJECT="$project" node --test scripts/browser/website.test.mjs ||
            fail "the browser check failed"
    fi
done
