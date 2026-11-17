# Release ctypst

A release is the Forgejo release flow of `ccid` (`.ci/ccid.toml`, `[release]`): the version comes from
`Cargo.toml`, one dispatch of the `release` workflow on `main` makes every file once and a green run publishes
the draft as `v<version>`. The registries and every copy carry those same bytes.

| Step | What runs |
| --- | --- |
| Build | `.ci/release-build.sh` calls `tools/release` (`ctypst-release prepare all`): locked quality and tests, the crate, the custom Wasm build with its packaged smoke test, the npm archive |
| Files of the release | `ctypst-<version>.crate`, the npm archive and one receipt per component, plus `SHA256SUMS` from the promotion |
| Publication | the datacenter-side publisher uploads the crate and the npm archive after checking them against `SHA256SUMS` |

Keep `Cargo.toml`, `Cargo.lock`, `js/@corbet-labs/ctypst/package.json` and `jsr.json` versions consistent.
Published contents are immutable: use a new version for corrections. A native Windows build, where one is
needed, is the linux windows-compile gate or the windows clone class, never a hosted runner.
Source identity: `ctypst-release source` (`tools/release`) prints the commit, archive and content identity the
build binds its receipts to.
