# ctypst

`ctypst` is the small native Typst substrate shared by CCVL and CareerVector.
It embeds the compiler and selected fonts, renders deterministic PDFs, exposes
Typst metadata queries and raster pages, and keeps product-specific document
rules out of the engine.

Add the latest compatible release from crates.io with only the capabilities
your application needs:

```toml
[dependencies]
ctypst = { version = "0.3.2", default-features = false, features = ["document-fonts", "pdf"] }
```

The default boundary is deliberately closed:

- no Typst executable, system fonts, package downloads, shell, or network;
- filesystem imports stay below one canonical root, including through links;
- Typst package imports are rejected;
- warnings fail compilation unless a caller explicitly accepts them;
- paths, sources, assets, fonts, inputs, pages, PDFs, and rasters have finite limits;
- request-scoped file overrides are compiled atomically and then rolled back;
- one engine instance reuses parsed fonts and Typst caches safely;
- PDF timestamps are explicit and default to the Unix epoch.

The library safely constrains I/O capabilities. It does not claim to make
hostile Typst programs safe inside the caller's process: templates are trusted
application code. Run untrusted templates in a separately limited process with
OS-enforced CPU, memory, time, and filesystem limits. See [SECURITY.md](SECURITY.md).

```rust
use ctypst::{CompileRequest, Engine, PageConstraint, fonts};

let engine = Engine::builder()
    .fonts(fonts::documents())
    .source("main.typ", "#set text(font: \"Archivo\")\nHello")?
    .build()?;

let output = engine.compile(
    CompileRequest::new("main.typ")
        .binary_file("profile.json", br#"{"name":"Ada"}"#.to_vec())
        .pages(PageConstraint::Exactly(1)),
)?;

let pdf = engine.pdf(&output.document, 0)?;
# Ok::<(), ctypst::Error>(())
```

Feature flags keep consumers lean:

- `document-fonts`: the complete document pack plus an Archivo-only subset;
- `format`: Typstyle source formatting;
- `pdf`: deterministic PDF export;
- `raster`: RGBA page rendering.

The crate is pure Rust at runtime and tested on Linux, macOS, and Windows. It
does not require a Typst installation or discover fonts from the host.

`Engine::compile_tracked` returns a `CompileReport` containing the usual
compilation `result` and its filesystem `dependencies`, including attempted
reads when compilation fails. Watchers can follow transitive Typst imports,
data and image reads without parsing source or guessing file extensions.
Reports retain lexical paths and allowed canonical targets for symlink changes,
and remain separate across cached or concurrent compilations. A failed path
may be missing or point outside the permitted root; watchers must apply the
same root boundary before reading it. Caller-supplied virtual files and font
bytes remain the caller's own dependencies. Attempted-path reports contain at
most twice `Limits::max_files` plus one final path for failure recovery; many
symlink aliases can exhaust this budget even when sharing a canonical target.

Developer and CI checks share `bash scripts/ci.sh all`. Individual targets are
`quality`, `test`, `python`, `javascript`, and `license`. The default Rust
toolchain is 1.92.0; `CI_RUST_TOOLCHAIN=system` explicitly tests the installed
compiler instead. Python checks need Python 3.11+ and uv; JavaScript checks use
Bun 1.3.13 and the committed dependency lock. These commands never publish.
JavaScript checks use a temporary source copy so installed dependencies and
generated assets do not enter the Rust package. Rust quality checks reject
package manifests containing dependency or virtual environment directories.

The manual Crow `verify` workflow verifies a staged source archive against its
commit and checksum, then runs the Linux checks with the worker's configured
compiler budget and persistent caches. Native macOS/Windows tests and the pinned Rust toolchain
remain separate GitHub Actions gates; a Crow Linux success does not prove them.
Set Crow's optional `CHECK_TARGET` variable to an individual target for a
focused rerun; it defaults to the locked Rust `test` target and rejects unknown
targets. Select `all` deliberately for full release preparation. The `quality`
target includes a Cargo publication dry run, while Python and license checks
may resolve packages from their registries; those lanes are not offline checks.
`CI_LINKER=mold` opts into an already installed mold linker for a measured run;
the default `system` keeps the toolchain's linker selection.
Crow calls the pinned shared `ccid` driver through `.ci/ccid.toml`; the driver
sets the allocated resources, project cache, lock, and timeout for these same targets.
Integration contracts share one test executable to avoid repeatedly linking
the embedded compiler; their individual tests and the library unit tests remain
independent checks.
