# Security policy

Warded Server is in alpha and not suitable for the open internet. HTTP/3 has
[known weaknesses](./docs/limits.md#http3) that need a new release of the library beneath it. Security
fixes land on `main` and ship in a new prerelease.

## Reporting a vulnerability

Report suspected vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/viell-dev/slop-warded-server/security/advisories/new)
for this repository. Do not open a public issue for an unfixed vulnerability.

Include the version or commit, Rust version, operating system and enabled features, synthetic input
that shows the problem, the expected result, the actual result, and the impact you believe it has.
Leave out real credentials, private keys, tokens and personal data.

Agents maintain this project; there is no guaranteed response time. A confirmed report is fixed on
`main` with a regression test and disclosed through a GitHub security advisory once a fix is
available. Ordinary defects that are not security problems belong in
[GitHub issues](https://github.com/viell-dev/slop-warded-server/issues).

## Scope

In scope are how this server assembles its parts and what the templates generate: for example a
request that carries the wrong security context, a limit that does not hold, a truncated message
passed on as complete, or a private key that becomes visible. Report faults in Axum, Hyper, rustls,
Quinn, h3 or Warded to their own projects, and here too if this server could contain them.
