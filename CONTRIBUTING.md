# Contributing

Warded Server is an early Rust project maintained by agents. Small, well-scoped reports, tests and
fixes are welcome. Establish scope before proposing a new framework integration, protocol or
unrelated feature. Describe behavior with synthetic requests and expected outcomes.

Use a branch and pull request against `main`. The required `ci` check runs on Linux and Windows:

- `bash scripts/check.sh` checks the library in each combination of features, with tests over real
  HTTP/1.1, HTTP/2 and HTTP/3 connections. Use Git Bash on Windows.
- `bash scripts/check-templates.sh` generates a project from every template, with the library from
  your checkout, and checks it. It needs `cargo-generate`.
- `cargo-deny --locked check` checks the dependencies.

Record the platforms, Rust version, commands and evidence limits in the pull request, and review the
complete change after its checks pass. The [design notes](./internals/design.md) give the reasons
behind the current structure, and [releases](./internals/release.md) the release procedure.

Report suspected vulnerabilities using the [private GitHub route](./SECURITY.md).

Agent-authored issues, pull requests and reviews must clearly disclose their authorship, the
submitting model and harness, submission on the account owner's behalf, and whether that person
reviewed the material. Authorization and automated or agent verification do not constitute human
review. Use an `Assisted-by: <model> via <harness>` commit trailer for agent assistance; do not add
agent `Co-Authored-By` or `Signed-off-by` trailers.

Keep one `Type:` label on every issue and pull request. An optional `Status:` label expresses the
current disposition; `Needs: Human` or `Needs: Owner` identifies a required response and must be
accompanied by an assignee and self-contained request. The [label catalog](./.github/labels.json)
defines them.

Contributions to original code are licensed under MIT OR Apache-2.0. Retain applicable notices and
provenance when adapting third-party source; original project licensing does not replace its terms.
