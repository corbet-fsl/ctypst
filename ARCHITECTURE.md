# ctypst unification architecture

Status: target architecture. The native engine exists; the shared measurement
protocol, dual runtime distribution, and consumer migrations do not.

## 1. Decision

Typst measurement is central document infrastructure. It MUST have one semantic
implementation, not several implementations kept approximately equivalent by
fixtures.

`ctypst` is the source of truth for:

- the product-neutral Typst measurement program;
- its versioned request and result protocol;
- the document font pack and its manifest;
- the native Rust adapter;
- the Typst-WASM adapter used by browser and Node runtimes; and
- cross-runtime conformance.

Consumers own only the translation between their product model and the shared
protocol, plus their product-specific policy. A consumer MUST NOT generate its
own measurement program, reproduce calibration or line derivation, or define a
parallel cache-key contract.

This unifies semantics, not compiler technology. Native applications continue
to use the Rust Typst engine. The browser continues to use Typst WASM. Native
and browser measurement execute the same versioned Typst program and exchange
the same data shape. The hosted compiler remains separately isolated, consumes
the same released assets, and participates in compiler-version conformance; it
does not remain a separate measurement API.

This architecture does not centralize product templates, CareerVector's Node
serializer, CCVL's document rules, output storage, or UI behavior. It also does
not claim to sandbox hostile Typst programs or move unique code merely to make
a consumer repository smaller.

## 2. Current state and failure mode

The native compiler boundary is already shared successfully by CCVL and the
CareerVector TUI through `ctypst::Engine`. Measurement above that boundary is
duplicated:

| Consumer | Runtime | Current implementation |
|---|---|---|
| CCVL | Rust + native Typst | `.agent/src/measure.rs`, `.agent/src/check.rs`, `.agent/typst/line-contract.typ` |
| CareerVector TUI | Rust + native Typst | `careervector-tui/ruler/src/lib.rs` and its conformance binary |
| CareerVector browser | TypeScript + Typst WASM | `measure-source.ts`, `measure-cache.ts`, and the measurement block in `typst-worker.ts` |
| Hosted `typst-compile` | Node + Typst WASM | an older page-break/SVG-height implementation behind unused `POST /measure` |

The CareerVector Rust and TypeScript paths separately implement the same DTOs,
escaping, Typst source generation, calibration, line derivation, character
budget, cache keys, and query decoding. Their mirrored vectors detect drift
after it happens, but do not remove the cause.

CCVL overlaps at the measurement primitive but also contains distinct product
policy: exact CV line sets, paragraph regions, fill bounds, failure wording,
and the `wrap-exact` packing algorithm. Those are not all shared concerns.

The same 16 font files are also checked into `ctypst`, CCVL, CareerVector's
browser assets, the hosted compiler, and the collected TUI application tree.
The copies are currently byte-identical but independently owned.

## 3. Target shape

```text
ctypst/
├── src/
│   ├── engine.rs                 existing native engine
│   ├── files.rs                  existing capability-limited VFS
│   ├── pdf.rs, raster.rs         existing native exports
│   └── measure.rs                Rust adapter, feature = "measure"
├── typst/
│   └── measure-v1.typ            the one measurement implementation
├── protocol/
│   └── measure-v1/
│       ├── schema.json           request and result schema
│       ├── requests.json         conformance inputs
│       ├── expected.json         canonical outputs
│       └── README.md             invariants and compatibility rules
├── js/
│   └── @corbet-labs/ctypst       Typst-WASM adapter and packaged assets
└── fonts/
    ├── manifest.json             names, faces, licenses, and SHA-256 hashes
    └── *.ttf                     canonical font bytes
```

The repository produces two bindings from one source tree:

| Artifact | Consumers | Responsibility |
|---|---|---|
| Rust crate `ctypst` | CCVL and CareerVector TUI | Engine plus typed native measurement adapter |
| npm package `@corbet-labs/ctypst` | CareerVector browser and `typst-compile` | Typst-WASM adapter, Typst asset, protocol types, and font manifest/assets |

Both artifacts MUST be cut from the same tag and carry the same protocol and
asset hashes. Consumer repositories use exact released versions. They do not
vendor the Typst source, mirror conformance files, use git submodules for the
shared layer, or float dependency ranges.

## 4. Measurement data flow

```text
product model
    │ product-owned mapping
    ▼
measure-v1 request data
    │ Rust or Typst-WASM adapter
    ▼
measure-v1.typ in the runtime VFS
    │ one compile + metadata query
    ▼
typed measure-v1 results
    │ product-owned policy
    ▼
overflow indicator, document gate, or advice
```

The request is data, not generated Typst source. Raw text and identifiers are
supplied through a JSON/VFS or system-input channel. `measure-v1.typ` owns the
single escaping and supported-markup policy before creating content. This
removes the Rust and TypeScript source builders and prevents their escaping
rules from diverging.

The protocol is product-neutral. A request describes the font, leading, and
items to measure. Each item carries an opaque ID, text, style, available width,
and any explicitly defined character-unit count needed to preserve the v1
character-budget behavior. It does not carry CV sections, CareerVector nodes,
CCVL paragraphs, companies, or application records.

The shared result contains the exact common facts needed by consumers:

- opaque item ID;
- natural width in points;
- wrapped height in points;
- derived line count;
- fill percentage; and
- character budget when requested by the protocol.

Calibration and line derivation execute inside `measure-v1.typ`. Repeating four
small calibration measurements on a cache miss is cheaper and safer than
maintaining equivalent arithmetic and calibration state in two host languages.

## 5. Runtime adapters

The Rust adapter lives in this crate behind a `measure` feature. It installs the
versioned Typst asset in the engine's virtual namespace, serializes the request,
compiles it, queries `<ctypst-measure-v1>`, validates every result, and returns
typed values. A separate `ctypst-measure` Rust crate would add a release and
dependency boundary without an independent lifecycle, so it is not justified.

The npm adapter performs the same transport and validation around Typst.js. It
may integrate with an existing compiler/world instance so the browser keeps its
worker, incremental compiler, and font initialization. It MUST NOT reimplement
measurement semantics in TypeScript.

Cache storage is runtime-local because browser and native lifetimes differ.
Cache semantics are simple and shared:

- key the exact serialized item and relevant format fields;
- include protocol version, Typst asset hash, font manifest hash, and compiler
  version;
- cache validated results only; and
- treat cache keys as implementation details, not cross-language behavior
  vectors.

Missing IDs, duplicate IDs, unknown protocol versions, non-finite dimensions,
incomplete query results, and compiler warnings MUST fail the request. No
adapter may silently omit a result or fall back to the former SVG/page-break
ruler.

## 6. Ownership boundary

| `ctypst` owns | Product repositories own |
|---|---|
| Typst measurement mechanics | Mapping product records/nodes/fields to measurement items |
| Safe text-to-content policy | Product IDs and the meaning of those IDs |
| Calibration and line derivation | Allowed line counts and fill ranges |
| Natural width, wrapped height, fill math | Region, paragraph, and document rules |
| Protocol labels and typed result validation | Failure/advice wording and UI presentation |
| Runtime-neutral conformance vectors | Templates, document wording, and record schemas |
| Font bytes, license metadata, and hashes | Output paths, persistence, publishing, and workflow |
| Engine limits and deterministic PDF export | Semantic PDF checks such as required text or claims |

This boundary deliberately leaves CareerVector's `ruler-items.ts` and TUI field
mapping in CareerVector. They understand the Node tree and application model.
It leaves CCVL's count, paragraph, density, and evidence gates in CCVL. A
generic numeric comparison would save little code while coupling `ctypst` to
product policy.

## 7. `wrap-exact` and other Typst layout helpers

`wrap-exact` stays entirely Typst-side because candidate measurement and
packing require the active layout context. It MUST NOT be split into a host
candidate generator plus per-candidate Typst calls.

It remains in CCVL while CCVL is its only consumer. Moving unique code into
this repository would relocate code without reducing the total surface. If a
second product needs the same exact packing behavior, graduate the complete
algorithm unchanged into a separately versioned `typst/layout-v1.typ` asset and
add cross-product conformance before migrating either consumer.

The same rule applies to text-layer inspection, byte-reproducibility checks,
and semantic PDF verification: keep them product-side until two consumers have
the same behavior. Page constraints, bounded output, and deterministic PDF
timestamps already belong to the engine.

## 8. Contracts and versioning

`ctypst` owns one immutable generic contract named `ctypst-measure-v1`. It
specifies observable measurement behavior, not generated source bytes or cache
key strings. A behavior change creates `ctypst-measure-v2`; implementation and
performance changes that preserve the vectors remain v1-compatible.

Product contracts remain with their products:

- CCVL retains `ccvl.json`, station gates, summary and paragraph budgets, and
  document-specific metric requirements.
- CareerVector retains tests that prove its Node-tree and TUI-model mappings
  produce the right generic measurement requests.

The mirrored `careervector-ruler-v1` Rust/TypeScript implementation contract is
a migration fixture, not a permanent authority. Once both consumers use
`ctypst-measure-v1`, delete its duplicated source snapshots, cache-key vectors,
and cross-repository parity machinery. The canonical conformance suite then
runs once in this repository against every supported runtime.

## 9. Fonts

`ctypst/fonts/` is the only source tree for the document font bytes. The Rust
crate embeds them. The npm package exposes bundler-friendly font assets and the
same manifest. The browser loads package-produced URLs, and `typst-compile`
resolves the package assets at build time.

Consumer repositories may retain product-specific font selection and fallback
policy, but MUST NOT retain copies of the canonical bytes. Legal bundles use
the packaged license and notice data rather than a second font tree. CI compares
the Rust and npm artifact manifests byte-for-byte.

## 10. Hosted compiler

The hosted compiler is a deployment/runtime concern, not another measurement
authority. Its unused `POST /measure`, measure cache, page-break program, and
SVG-height parser are removed. The active PDF compile path remains until its
callers are migrated or a separately sandboxed replacement exists.

The service consumes the exact `@corbet-labs/ctypst` asset version and pins its
Typst.js dependencies exactly. Browser and hosted compiler should use the same
underlying Typst release where available; unavoidable runtime version skew is
explicit in the conformance matrix and cannot ship when it changes canonical
results.

Do not casually replace the hosted WASM process with an in-process native
`ctypst` HTTP server. This crate constrains I/O but does not sandbox hostile
Typst programs. A native hosted service requires separate OS-enforced CPU,
memory, time, and filesystem limits.

## 11. Release gate

Every `ctypst-measure-v1` release runs the same requests through:

1. the Rust adapter with the native Typst engine;
2. the npm adapter with the browser Typst-WASM version; and
3. the npm adapter with the hosted compiler's Typst-WASM version while that
   version differs.

The gate compares typed results with explicitly documented numeric tolerances.
It also verifies that Rust and npm artifacts contain identical Typst and font
hashes. The gate runs in Crow because it spans the complete controlled runtime
matrix.

Consumer CI additionally rejects reintroduction of owned implementation names
such as a local `buildMeasureAllSource`, local calibration/line derivation, or
a product-specific copy of `<ctypst-measure-v1>`.

## 12. Migration and deletion order

1. Freeze the current CareerVector native/browser results and CCVL showcase
   metrics as migration inputs. Classify intentional product differences.
2. Implement `measure-v1.typ`, its schema, and conformance vectors in this
   repository. Prove the vectors against native and both WASM versions.
3. Publish the Rust and npm adapters from one tag with matching asset hashes.
4. Migrate CareerVector browser and TUI together. In the same change, delete
   the standalone ruler implementation, source builder, calibration and cache
   helpers, mirrored contract, conformance binary, and parity machinery.
5. Migrate CCVL's common measurement primitive and query decoding. Keep its
   product contracts, validation, `wrap-exact`, and document checks local.
6. Replace consumer font trees with the released font artifact and delete the
   redundant bytes after package/license verification.
7. Remove the hosted service's unused measurement and SVG paths, align its
   compiler and shared-asset versions, and run the deployed PDF smoke.
8. Remove obsolete documentation and add ownership checks so convergence is a
   completed migration, not a long-lived compatibility layer.

## 13. Completion criteria

The unification is complete only when:

- one source-owned measurement Typst program is used by every current
  consumer;
- no product repository implements measurement escaping, calibration, line
  derivation, or measurement source generation;
- CareerVector no longer contains the standalone `careervector-ruler` crate or
  mirrored ruler contract;
- the hosted compiler has no independent measurement endpoint;
- canonical font bytes occur in only this source repository;
- all consumers pin released adapters and fail on protocol mismatch; and
- native, browser, and hosted conformance plus product behavior tests pass.
