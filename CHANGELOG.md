# Changelog

All notable changes to `ctypst` are documented here. The project follows
Semantic Versioning.

## 0.3.1 - 2026-09-06

- Shrink the browser WebAssembly build by a quarter with size-tuned
  codegen and `wasm-opt -Oz`; the release gates the artifact on a 25 MiB
  distribution budget.

## 0.3.0 - 2026-09-05

- Add per-page SVG export behind the `svg` Cargo feature.
- Add the `wasm` Cargo feature: the whole runtime (engine, measurement,
  exporters, embedded fonts) as a `wasm-bindgen` boundary for browsers
  and Node, so every runtime executes the same code.

## 0.2.0 - 2026-09-05

- Add the versioned measurement protocol (`ctypst-measure-v1`): one shared
  Typst measurement program with structured JSON input, frozen
  cross-runtime conformance vectors, and the canonical font manifest with
  a drift test.
- Add the native measurement adapter behind the `measure` Cargo feature:
  typed requests, result caching with compile-count transparency,
  fail-loud validation, calibration observability, and the frozen UTF-16
  character-budget formula.
- Restore the full document font pack (Archivo, EB Garamond, IBM Plex
  Serif, Source Serif 4): every consumer format needs its family, so the
  single source tree carries all sixteen faces again.

## 0.1.1 - 2026-09-04

- Reduce the embedded `document-fonts` feature to Archivo Regular, Medium,
  Bold, and Italic.
- Remove the twelve unused font assets from the published crate.

## 0.1.0 - 2026-09-04

- Introduce reusable embedded Typst compilation with bounded filesystem and
  virtual-file capabilities.
- Add deterministic PDF export, JSON metadata queries, RGBA rasterization, and
  Typstyle formatting behind independent Cargo features.
- Bundle the OFL-licensed Archivo, EB Garamond, IBM Plex Serif, and Source Serif
  4 font families, including an Archivo-only subset for measurement workloads.
- Enforce request-scoped virtual-file rollback, finite resource limits, denied
  package imports, canonical-root containment, and warnings-as-errors defaults.
- Test feature boundaries and native execution on Linux, macOS, and Windows.
