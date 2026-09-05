# ctypst

`ctypst` is the small native Typst substrate shared by CCVL and CareerVector.
It embeds the compiler and selected fonts, renders deterministic PDFs, exposes
Typst metadata queries and raster pages, and keeps product-specific document
rules out of the engine.

Add the latest compatible release from crates.io with only the capabilities
your application needs:

```toml
[dependencies]
ctypst = { version = "0.1.1", default-features = false, features = ["document-fonts", "pdf"] }
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
