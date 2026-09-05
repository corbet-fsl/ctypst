# ctypst-measure-v1 protocol

Versioned request/result contract for the one shared Typst measurement
program (`typst/measure-v1.typ`). Lower ring than any consumer memory:
when this file and a consumer fixture disagree, this file wins.

## Files

- `schema.json` — JSON Schema for requests; result shapes under
  `definitions` (`resultItem`, `calibration`).
- `requests.json` — conformance inputs: named requests with format and
  items (escaping torture, `*`/`_` markup, Unicode/emoji, empty text,
  multi-line wraps, serif + us-letter variants).
- `expected.json` — canonical outputs (per-item `w`/`h`/`lines` plus the
  calibration record), frozen from the native Typst engine. Native
  determinism envelope: ids, counts, and line numbers match bitwise;
  floats match within one ulp (a few values sit exactly on IEEE rounding
  boundaries and wobble by one ulp across hosts and builds — fourteen
  orders of magnitude below any product threshold).
- `README.md` — this file.

## Invariants

- The request is data, never generated Typst source. Raw text travels
  verbatim; `measure-v1.typ` owns escaping and the supported-markup
  policy (`*strong*`, `_emphasis_`, everything else literal).
- Calibration (four probes) and line derivation execute inside the
  program on every compile that needs them. Hosts never recompute them.
- Character budgets stay host-side: they need host UTF-16 string
  semantics, which Typst strings do not provide. Adapters implement the
  frozen formula and cover it with unit tests.
- Cache keys are runtime-local implementation details (exact serialized
  item + format fields + protocol/asset/font/compiler versions). They are
  not cross-language vectors.
- Failures are loud: unknown protocol version, empty or duplicate item
  IDs, unknown weights, non-finite or non-positive dimensions, missing
  results, and compiler warnings all fail the request. No adapter may
  silently omit a result or fall back to a former ruler.

## Compatibility rules

- An observable behavior change (different `w`/`h`/`lines`, escaping, or
  derivation for any vector) creates `ctypst-measure-v2`: a new program
  asset, a new protocol directory, and new vectors. Both versions may
  coexist under `/ctypst/`.
- Implementation and performance changes that preserve every vector
  remain v1-compatible (new patch/minor crate releases).
- The cross-runtime gate runs these vectors through the Rust adapter
  (native engine) and the npm adapter (each supported Typst-WASM
  version). Native must match exactly; WASM results are compared with
  the tolerance documented by the gate. A version skew that changes
  canonical results cannot ship.
