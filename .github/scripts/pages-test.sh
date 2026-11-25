#!/usr/bin/env bash
# Tests pages-resolve.sh against a fake gh command.
set -euo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
cd "$scratch"

tag_commit=1111111111111111111111111111111111111111
branch_commit=2222222222222222222222222222222222222222
export GITHUB_REPOSITORY=example/server GITHUB_OUTPUT="$scratch/output"
export TEST_RELEASES="$scratch/releases" TEST_COMPARE="$scratch/compare"
export TEST_TAG_COMMIT="$tag_commit" TEST_BRANCH_COMMIT="$branch_commit"

mkdir bin
export PATH="$scratch/bin:$PATH"
# gh api <path> [--jq <filter>] [--paginate], answering from the test files and variables.
cat > bin/gh <<'BASH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]] || exit 1
shift
filter=. path=
while (($#)); do
    case "$1" in
        --jq) filter="$2"; shift 2 ;;
        --paginate) shift ;;
        *) path="$1"; shift ;;
    esac
done
case "$path" in
    "repos/$GITHUB_REPOSITORY/releases") body="$(cat "$TEST_RELEASES")" ;;
    "repos/$GITHUB_REPOSITORY/commits/refs/tags/0.1.0-alpha.10")
        body="{\"sha\":\"$TEST_TAG_COMMIT\"}" ;;
    "repos/$GITHUB_REPOSITORY/commits/refs/heads/site-0.1.0-alpha.10")
        [[ -z "${TEST_NO_BRANCH:-}" ]] || exit 1
        body="{\"sha\":\"$TEST_BRANCH_COMMIT\"}" ;;
    "repos/$GITHUB_REPOSITORY/compare/$TEST_TAG_COMMIT...$TEST_BRANCH_COMMIT")
        body="$(cat "$TEST_COMPARE")" ;;
    *) exit 1 ;;
esac
jq -r "$filter" <<< "$body"
BASH
chmod +x bin/gh

releases() {
    printf '%s\n' "$@" | jq -Rn '[inputs | {tag_name: ltrimstr("draft:"),
        draft: startswith("draft:")}]' > "$TEST_RELEASES"
}

compare() {
    jq -n --arg status "$1" '{status: $status, files: $ARGS.positional | map({filename: .})}' \
        --args "${@:2}" > "$TEST_COMPARE"
}

resolve() {
    : > "$GITHUB_OUTPUT"
    TAG="$1" CORRECTIONS="${2:-false}" bash "$scripts/pages-resolve.sh" > log 2>&1
}

passes() {
    local expected="$1"
    shift
    resolve "$@" || { echo "refused: $*" >&2; cat log >&2; exit 1; }
    grep --quiet --line-regexp "commit=$expected" "$GITHUB_OUTPUT" ||
        { echo "chose another commit: $*" >&2; cat "$GITHUB_OUTPUT" >&2; exit 1; }
}

refuses() {
    local reason="$1"
    shift
    if resolve "$@"; then
        echo "accepted: $*" >&2
        exit 1
    fi
    grep --quiet --fixed-strings "$reason" log ||
        { echo "refused for another reason: $*" >&2; cat log >&2; exit 1; }
    [[ ! -s "$GITHUB_OUTPUT" ]] || { echo "wrote an output: $*" >&2; exit 1; }
}

releases 0.1.0-alpha.9 0.1.0-alpha.10 0.1.0-alpha.2 draft:0.1.0-alpha.11 nightly
passes "$tag_commit" 0.1.0-alpha.10
refuses "not the newest" 0.1.0-alpha.9
refuses "not the newest" 0.1.0-alpha.11
refuses "not an unprefixed" v0.1.0-alpha.10
refuses "not an unprefixed" ""
refuses "must be true or false" 0.1.0-alpha.10 maybe
releases 0.1.0-alpha.10 0.1.0-beta.1
refuses "not the newest" 0.1.0-alpha.10
releases 0.1.0-alpha.10 0.1.0
refuses "not the newest" 0.1.0-alpha.10
echo '[]' > "$TEST_RELEASES"
refuses "there is none" 0.1.0-alpha.10

releases 0.1.0-alpha.10
compare ahead docs/index.md docs/.vitepress/config.mts
passes "$branch_commit" 0.1.0-alpha.10 true
compare ahead docs/index.md src/lib.rs
refuses "outside docs/: src/lib.rs" 0.1.0-alpha.10 true
jq -n '{status: "ahead", files: [{filename: "docs/a.md", previous_filename: "a.md"}]}' \
    > "$TEST_COMPARE"
refuses "outside docs/: a.md" 0.1.0-alpha.10 true
compare diverged docs/index.md
refuses "does not descend" 0.1.0-alpha.10 true
TEST_NO_BRANCH=1 refuses "does not exist" 0.1.0-alpha.10 true

echo "Pages resolution tests passed"
