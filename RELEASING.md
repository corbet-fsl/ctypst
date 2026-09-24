# Release ctypst

Prefer eligible, available GitHub Actions. Use the manual Crow `release`
workflow when hosted execution is unavailable or publication credentials must
remain on owned infrastructure. Both providers call `scripts/release.py`.
Neither command changes registry authentication policy or installs worker tools.

| Stage | Shared command | Result |
| --- | --- | --- |
| Verify submitted source | `python3 scripts/release.py source` | Actual complete source inventory, modes, commit and archive identity, without building or requiring artifacts |
| Prepare Cargo | `python3 scripts/release.py prepare cargo` | Locked quality/tests and dry-run crate, or an explicitly verified retained crate |
| Prepare JavaScript | `python3 scripts/release.py prepare javascript` | TypeScript/conformance checks, packaged WASM smoke, npm archive, JSR dry run/archive |
| Inspect Cargo | `python3 scripts/release.py status cargo` | Exact existing registry bytes, without uploading |
| Preflight | `python3 scripts/release.py preflight all` | Source/tag/artifact checks, registry reconciliation and selected credential availability |
| Publish | `python3 scripts/release.py publish all` | Exact Cargo/npm/JSR uploads, complete GitHub draft assets, then publication and public download verification |
| Check release plumbing | `bash scripts/ci.sh release-guards` | Offline failure fixtures, actionlint and ShellCheck; no Rust build or publication |

`all` includes both package components and the complete GitHub release.
`cargo` publishes only crates.io; `javascript` publishes npm and JSR.
All stages require `RELEASE_TAG=v<committed Cargo version>`.
GHA uses a credential-free preparation job and transfers a hashed package bundle
to its publication job. It selects moving stable Rust. Crow uses the provisioned
compiler and pinned shared ccid source/binary, with explicit memory admission,
bounded jobs, locked dependencies and the actual persistent Cargo target lock.

## Crow dispatch and prerequisites

Use the shared Crow submitter with the committed `.crow/release.yaml` workflow.
The same source-archive/hash, ccid-archive/hash/binary and resource arguments as
`verify` apply. The manual workflow accepts:

| Variable | Meaning |
| --- | --- |
| `RELEASE_TAG` | Existing immutable tag matching the package version |
| `RELEASE_COMPONENT` | `cargo`, `javascript`, or `all` (default) |
| `RELEASE_STAGE` | `prepare`, `publish`, or `all` (default) |
| `RELEASE_ARTIFACT_DIR` | Optional absolute location of retained package receipts, bytes and publication journals |
| `RELEASE_REUSE_CRATE` | Optional existing verified Cargo archive; see retained-artifact import below |

The default artifact location is
`$CARGO_TARGET_DIR/releases/<canonical-source-content-sha256>`.
Preparation reuses matching receipts and verified bytes. Source identity includes
the commit, every source path, executable mode, file checksum and Cargo.lock.
The complete working input inventory must equal its verified committed archive.
Transport tar checksums remain in provenance; harmless GHA/Crow tar encoding
differences do not invalidate identical source content.

Configure these repository secrets for **manual events only**, scoped to ctypst:

| Secret | Publication environment |
| --- | --- |
| `ctypst_cargo_token` | `CARGO_REGISTRY_TOKEN` |
| `ctypst_npm_token` | `NPM_TOKEN` |
| `ctypst_jsr_token` | `JSR_TOKEN` |
| `ctypst_github_token` | `GH_TOKEN` |

Crow selects steps before resolving secrets. `RELEASE_STAGE=prepare` needs no
publication credentials. Cargo publication requires only its Cargo token;
its immutable public tag is verified through the public GitHub API.
JavaScript requires GitHub, npm and JSR tokens; `all` requires all four. Invalid
selectors fail in an unconditional validation step. This lets preparation and
individual components run when unrelated registry credentials are absent.

Secrets appear only in the selected publication step. Preparation refuses
publication tokens. npm receives only its own token and publishes the prepared archive with
lifecycle scripts disabled. Crow does not claim hosted OIDC provenance. The
GHA route uses configured short-lived trusted authentication. Long-lived
registry tokens remain on Crow; neither route configures trusted publishers
or makes them exclusive.

Cargo token publication requires the registry's `trustpub_only` setting to be
false. If it is true, the command fails before upload and explains the missing
publishing permission; it never weakens or restores this setting itself.
Credential presence does not prove every registry-side permission: successful
destination operations and exact public downloads supply that final evidence.

Cargo preparation needs existing Rust, Cargo, Clippy and rustfmt. JavaScript
preparation additionally needs Bun, Deno, wasm-opt, the installed
`wasm32-unknown-unknown` target, and wasm-bindgen matching Cargo.lock. Locked
project JavaScript dependencies are installed in job scratch. Publication needs
Python 3.11+, npm and Deno, without compilers or dependency installers. Missing
tools fail visibly; owned workers never run rustup/tool-install commands.

## Retained Cargo artifact import

The importer consumes the exact old archive without rebuilding it. Supply
`RELEASE_REUSE_CRATE`, `RELEASE_REUSE_SOURCE_ARCHIVE`, `RELEASE_REUSE_RECEIPT`
and `RELEASE_REUSE_RECEIPT_SHA256`. The receipt is a reviewed JSON object:

```json
{
  "version": "0.3.2",
  "producing_commit": "<full producing Git commit>",
  "tag_commit": "<full existing immutable tag commit>",
  "source_sha256": "<producing source archive SHA-256>",
  "crate_sha256": "<verified crate SHA-256>",
  "checks_passed": true,
  "evidence": ["<immutable successful job and artifact evidence>"]
}
```

The operator must verify that evidence before supplying its receipt hash; the
importer does not invent a validation receipt. It checks the source archive's
commit, every crate payload against that source, normalized Cargo metadata,
the locked graph, and the full current runtime-input inventory. The package
receipt distinguishes the new release-driver source from the old producing
source and records the reviewed tag discrepancy.

The retained 0.3.2 crate produced by `330f235` and the historic `v0.3.2` tag at
`2d0b5c8` are distinct. Recovery is **Cargo-only** through this explicit import.
The reviewed archive hashes, original producing commit, unchanged tag and
successful Crow checks are recorded in
[the retained import receipt](.ci/release-imports/ctypst-0.3.2.json). Supply that
file and its SHA-256 as the import receipt when reusing those exact bytes.
It cannot be combined into `publish all` with newly built JavaScript from a
different tag identity. The historical tag also predates the shared release
driver, so the current GHA workflow cannot execute that driver after checking
out the old tag. Its early check reports this limitation. Preserve the tag;
decide the next coherent version/tag for the WASM packaging changes before
preparing a complete new release.

## Partial publication and provider fallback

Each destination operation records and fsyncs an intent journal **before** its
first upload. An existing version must match the prepared bytes; conflicts fail.
An existing journal with missing or uncertain remote results prohibits another
upload. A later invocation reconciles the same journal read-only first. Nothing
overwrites a registry version, edits release notes or clobbers an asset.

Publication order is Cargo, JSR, npm, then the GitHub release. npm runs last
among the registries so that an npm failure (for example while its trusted
publishing rule is still missing) cannot block Cargo or JSR; the run then fails
and the GitHub release waits until npm is reconciled.

GitHub release creation uses a draft. Asset inventories and downloaded bytes are
verified before making it public; a published release with missing/extra assets
is refused without alteration. Public downloads are then verified again.

Preserve the artifact directory, source identity and journals across fallback.
Download the original GHA bundle and publication-journal artifacts into one
Crow artifact directory; verify the bundle checksum recorded by the original
preparation job, then use the exact same committed source and `RELEASE_STAGE=publish`.
Different archive encodings are accepted only when their canonical input
identity matches. Never discard an ambiguous attempt's journal to obtain a
fresh upload. If a hosted runner disappeared before retaining its journal,
reconcile the destination and original run explicitly before any new upload.
The GHA publisher refuses reruns and earlier release runs for the same source,
because an ephemeral new workspace cannot prove that no previous upload occurred.

## Remaining platform and Python scope

This route covers the existing Cargo/npm/JSR/GitHub release outputs. Crow's
Linux result does not replace native Windows or macOS validation. A full native
fallback requires corresponding available native executors.

`bindings/python` contains a maturin/PyO3 package and its tests, but no existing
release job builds wheels/sdist or publishes them to PyPI. A Python release needs
a defined supported interpreter/platform matrix, provisioned native build tools,
installed-wheel conformance checks, wheel/sdist receipts and PyPI authentication.
The commands above do not label Python distribution as complete.
