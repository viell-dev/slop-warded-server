---
description: "How Warded Server numbers its releases, what alpha means, and which documentation belongs to which version."
---

# Versions

Warded Server uses [Semantic Versioning](https://semver.org) with prerelease stages. This site
describes the version in the navigation bar.

- **Alpha:** the API, the settings and the templates can change in any release. It is in alpha now.
- **Beta:** the agreed features are complete, and work turns to fixes.
- **Release candidate:** a build that is meant to become the stable release unchanged.

## Where it is released

Releases are tags of [the repository](https://github.com/viell-dev/slop-warded-server/releases). It
is not on crates.io. Depend on the exact tag, so that you choose when to move:

```toml
[dependencies]
warded-server = { git = "https://github.com/viell-dev/slop-warded-server", tag = "0.1.0-alpha.2" }
```

The server brings the version of Warded that it was tested with: 0.1.0-beta.5.

## Documentation

- This site describes the latest release. Its `version.json` names the commit it was built from.
- The [API reference](https://viell-dev.github.io/slop-warded-server/api/warded_server/) here is for
  the same release. For another one, run `cargo doc --open` in a project that uses it.
- Unreleased changes are in the
  [changelog](https://github.com/viell-dev/slop-warded-server/blob/main/CHANGELOG.md) on the default
  branch.

## Rust

The minimum is 1.99.1, because Warded needs it. A newer minimum can come with any alpha.

## Security fixes

They land on the default branch and ship in a new release. The [security
policy](https://github.com/viell-dev/slop-warded-server/security/policy) says how to report a
vulnerability.
