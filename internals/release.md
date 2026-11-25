# Releases

Releases are Git tags with a GitHub release; no package registry is an agreed channel. Versions are
SemVer prereleases without a `v` prefix, and alpha lasts while features, configuration or templates
still change.

1. On a branch, set the version in `Cargo.toml`, in the `tag` of every
   `templates/*/Cargo.toml.liquid`, in the README and on the site's pages. Give the changelog's
   section the version and date. `scripts/check.sh` fails while they differ.
2. Run the checks below on Linux, and record them in the pull request.
3. Have the changes since the previous release reviewed independently, and settle the findings.
4. After the pull request is merged and CI has passed on `main`, tag exactly that commit and push the
   tag.
5. Create the GitHub release from the tag, marked as a prerelease, with the changelog section.
6. Run the Pages workflow on `main` with the tag: `gh workflow run pages.yml -f tag=<version>`. Check
   `version.json`, the guides and the API reference on the site.
7. In an empty directory, follow the site's commands with the tag, and run each generated project.

A tag is never moved. Fix a faulty release with a new version.

## Correcting the released site

A correction to the documentation of a release needs no new version. Make it on `main` first. Then
create `site-<version>` from the tag, apply it there by pull request, changing only `docs/`, and run
`gh workflow run pages.yml -f tag=<version> -f corrections=true`.

## Checks that CI does not run

With the `hello` example running, and a certificate that Chromium can be told to accept:

- curl with `--http1.1`, `--http2` and `--http3-only`;
- curl with `--alt-svc` and a file, twice: the second request uses HTTP/3;
- an upload of 30 MB, which the example does not read, with curl over each protocol and with `fetch` in
  Chromium: each gets its response, every time;
- Chromium with the flags from [the HTTP/3 page](../docs/http3.md#in-browsers): the document's
  `performance.getEntriesByType("navigation")[0].nextHopProtocol` is `h3`.

When they change, also the [deployment](../docs/deployment.md) recipes: the systemd unit with a reload
and a stop, and the container image from another computer.
