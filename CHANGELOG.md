# Changelog

All notable changes to `ctypst` are documented here. The project follows
Semantic Versioning.

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
