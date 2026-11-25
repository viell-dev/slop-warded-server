# Releases

Releases are Git tags with a GitHub release; no package registry is an agreed channel. Versions are
SemVer prereleases without a `v` prefix, and alpha lasts while features, configuration or templates
still change.

1. On a branch, set the version in `Cargo.toml`, in the `tag` of every `templates/*/Cargo.toml`, and
   in the README. Give the changelog's unreleased section the version and date.
   `scripts/check.sh` fails while they differ.
2. Run the checks below on Linux, and record them in the pull request.
3. Have the changes since the previous release reviewed independently, and settle the findings.
4. After the pull request is merged and CI has passed on `main`, tag exactly that commit and push the
   tag.
5. Create the GitHub release from the tag, marked as a prerelease, with the changelog section.
6. In an empty directory, follow the README's commands with the tag, and run each generated project.

A tag is never moved. Fix a faulty release with a new version.

## Checks that CI does not run

With the `hello` example running, and a certificate that Chromium can be told to accept:

- curl with `--http1.1`, `--http2` and `--http3-only`;
- curl with `--alt-svc` and a file, twice: the second request uses HTTP/3;
- Chromium with the flags from [configuration](../docs/configuration.md#http3): the document's
  `performance.getEntriesByType("navigation")[0].nextHopProtocol` is `h3`.
