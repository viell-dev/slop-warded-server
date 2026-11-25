#!/usr/bin/env bash
# Choose what the Pages workflow builds: a release tag, or the documentation corrections on that
# release's site-<version> branch. The site describes the latest release, so the tag must be the
# newest version among the repository's releases.
#
# Environment: TAG, CORRECTIONS (true or false), GITHUB_REPOSITORY and GITHUB_OUTPUT, with
# GH_TOKEN for gh.
set -euo pipefail

fail() {
    echo "$*" >&2
    exit 1
}

summary() {
    echo "$*"
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        echo "$*" >> "$GITHUB_STEP_SUMMARY"
    fi
}

tag="${TAG:-}"
corrections="${CORRECTIONS:-false}"
version='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
[[ "$tag" =~ $version ]] || fail "'$tag' is not an unprefixed version tag"

# Semantic Versioning precedence: a release outranks its prereleases, and prerelease identifiers
# compare numerically when they are numbers, below any alphanumeric identifier.
newest="$(gh api --paginate "repos/$GITHUB_REPOSITORY/releases" \
    --jq '.[] | select(.draft | not) | .tag_name' | jq -Rrn --arg version "$version" '
    def precedence:
        capture("^(?<major>[0-9]+)\\.(?<minor>[0-9]+)\\.(?<patch>[0-9]+)(-(?<pre>.+))?")
        | [(.major, .minor, .patch | tonumber),
           (if .pre == null then [1]
            else [0] + (.pre | split(".") |
                map(if test("^[0-9]+$") then [0, tonumber] else [1, .] end))
            end)];
    [inputs | select(test($version))] | max_by(precedence) // empty')" ||
    fail "Could not read the releases"
[[ "$newest" == "$tag" ]] || fail "$tag is not the newest release, ${newest:-of which there is none}"

# The commits API needs the fully qualified ref: it answers tags/<tag> with HTTP 422.
tag_commit="$(gh api "repos/$GITHUB_REPOSITORY/commits/refs/tags/$tag" --jq .sha)" ||
    fail "The tag $tag does not exist"

commit="$tag_commit"
if [[ "$corrections" == true ]]; then
    # Corrections must build on the release and change nothing but its documentation, so the
    # site never describes unreleased behavior.
    branch="site-$tag"
    commit="$(gh api "repos/$GITHUB_REPOSITORY/commits/refs/heads/$branch" --jq .sha)" ||
        fail "The branch $branch does not exist"
    comparison="$(gh api "repos/$GITHUB_REPOSITORY/compare/$tag_commit...$commit")" ||
        fail "Could not compare $branch with $tag"
    relation="$(jq -r .status <<< "$comparison")"
    case "$relation" in
        ahead | identical) ;;
        *) fail "$branch ($commit) does not descend from $tag ($tag_commit): $relation" ;;
    esac
    # The comparison lists at most 300 files.
    (($(jq '.files | length' <<< "$comparison") < 300)) || fail "$branch changes too many files"
    # A file renamed into docs/ also removes its old path.
    outside="$(jq -r '.files[] | .filename, (.previous_filename // empty) |
        select(startswith("docs/") | not)' <<< "$comparison")"
    [[ -z "$outside" ]] || fail "$branch changes files outside docs/: ${outside//$'\n'/, }"
    summary "$branch ($commit) holds documentation corrections on $tag."
elif [[ "$corrections" != false ]]; then
    fail "CORRECTIONS must be true or false, not '$corrections'"
fi

summary "Building the site of $tag, tag commit $tag_commit, from commit $commit."
printf 'tag-commit=%s\ncommit=%s\n' "$tag_commit" "$commit" >> "$GITHUB_OUTPUT"
