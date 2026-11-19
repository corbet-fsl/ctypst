/**
 * Shared measurement protocol types for `ctypst-measure-v1`.
 *
 * Mirrors the Rust adapter (`ctypst::measure`) and
 * `protocol/measure-v1/schema.json`. The request is data, never generated
 * Typst source: escaping, markup, calibration, and line derivation run
 * inside `measure-v1.typ` on every runtime.
 */

/** Versioned measurement contract served by the adapters. */
export const PROTOCOL_VERSION = 'ctypst-measure-v1' as const;

/** Typst query selector carrying every measurement result. */
export const QUERY_SELECTOR = '<ctypst-measure-v1>' as const;

/** Virtual path of the shared measurement program. */
export const PROGRAM_PATH = '/ctypst/measure-v1.typ' as const;

/** Virtual path of the per-compile JSON request. */
export const REQUEST_PATH = '/ctypst/request.json' as const;

/** Text weight carried by a measurement item. */
export type Weight = 'regular' | 'bold';

/**
 * One fragment to measure: opaque id, raw text, style, available width.
 * Text travels verbatim; the program owns escaping and `*`/`_` markup.
 */
export interface MeasureItem {
    /** Opaque caller id, echoed back. Unique per call. */
    id: string;
    /** Raw fragment text, possibly empty. */
    text: string;
    /** Font size in points. Finite and positive. */
    fontSize: number;
    /** Text weight. */
    weight: Weight;
    /** Available wrap width in points. Finite and positive. */
    usableWidthPt: number;
}

/**
 * Product format mapping. Fields are optional at the boundary and fall
 * back to the historical ruler defaults; the adapter normalizes them
 * into the canonical request shape.
 */
export interface MeasureFormatInput {
    font?: string;
    baseFontSize?: number;
    entryHeadingSize?: number;
    leading?: { value: number; relative: boolean };
    marginLeft?: number;
    marginRight?: number;
    pageSize?: string;
}

/** Normalized format as sent to the program. */
export interface NormalizedFormat {
    font: string;
    baseFontSize: number;
    entryHeadingSize: number;
    leadingEm: number;
    marginLeft: number;
    marginRight: number;
    pageSize: string;
}

/** Calibration ratios from the four Typst probes. Observability only. */
export interface MeasureCalibration {
    capRatioRegular: number;
    advanceRatioRegular: number;
    capRatioBold: number;
    advanceRatioBold: number;
}

/** One measured fragment: exact Typst facts plus the host-side budget. */
export interface MeasureResult {
    /** Echoed opaque caller id. */
    id: string;
    /** Natural (unwrapped) width in points. */
    widthPt: number;
    /** Wrapped height in points. */
    heightPt: number;
    /** Derived integer line count, at least one. */
    lines: number;
    /**
     * Character budget: positive room left, negative overflow, null when
     * the fragment is empty or has no measurable width. Uses host UTF-16
     * string semantics, which Typst strings do not provide.
     */
    charBudget: number | null;
}

/** Raw per-item program output before host enrichment. */
export interface RawResult {
    id: string;
    w: number;
    h: number;
    lines: number;
}
