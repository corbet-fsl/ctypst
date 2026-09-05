/**
 * Typst-WASM adapter for the versioned measurement protocol.
 *
 * Mirrors the Rust adapter (`ctypst::measure`): typed requests over the
 * one shared program, result caching that compiles only miss batches,
 * fail-loud validation, calibration observability, and the frozen UTF-16
 * character-budget formula.
 *
 * The adapter owns no compiler: pass an initialized Typst compiler (the
 * browser keeps its worker, incremental compiler, and font setup) and it
 * performs transport, validation, and caching around it. It never
 * reimplements measurement semantics in TypeScript.
 */
import type { TypstCompiler } from '@myriaddreamin/typst.ts';
import { MEASURE_V1_TYP, MANIFEST_JSON, PACKAGE_VERSION } from './generated/measure-asset.js';
import {
    PROTOCOL_VERSION,
    QUERY_SELECTOR,
    PROGRAM_PATH,
    REQUEST_PATH,
    type MeasureCalibration,
    type MeasureFormatInput,
    type MeasureItem,
    type MeasureResult,
    type NormalizedFormat,
    type RawResult,
} from './protocol.js';

/** Cache entry: validated result plus its canonical key inputs. */
interface CachedResult extends MeasureResult {
    key: string;
}

/** Minimal compiler surface the adapter needs (subset of TypstCompiler). */
export interface MeasureCompiler {
    addSource(path: string, content: string): void;
    runWithWorld<T>(config: { mainFilePath: string }, fn: (world: MeasureWorld) => Promise<T>): Promise<T>;
}

/** Minimal world surface the adapter needs. */
export interface MeasureWorld {
    compile(): Promise<unknown>;
    query<T>(query: { selector: string; field: string }): Promise<T>;
}

function fail(what: string): Error {
    return new Error(`invalid ${PROTOCOL_VERSION} request: ${what}`);
}

/** FNV-1a over bytes, matching the Rust adapter's cache-key hashing. */
export function fnv1a64Hex(data: Uint8Array | string): string {
    const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
    let hash = 0xcbf29ce484222325n;
    for (const byte of bytes) {
        hash ^= BigInt(byte);
        hash = BigInt.asUintN(64, hash * 0x100000001b3n);
    }
    return hash.toString(16).padStart(16, '0');
}

/**
 * Canonical em multiplier: relative values pass through, point values
 * divide by the base size to four decimals. Mirrors the Rust adapter and
 * the historical ruler rule.
 */
export function leadingEm(leadingValue: number, leadingRelative: boolean, baseFontSize: number): number {
    if (leadingRelative) return leadingValue;
    return +(leadingValue / baseFontSize).toFixed(4);
}

/**
 * Fragment-level character budget. Positive room left, negative overflow,
 * null when empty or unmeasurable. Uses UTF-16 code units, matching the
 * Rust adapter and the historical ruler formula.
 */
export function charBudget(text: string, widthPt: number, usableWidthPt: number): number | null {
    if (text.length === 0) return null;
    if (widthPt <= 0) return null;
    return Math.round(text.length * (usableWidthPt / widthPt - 1));
}

/** Normalize a product format with the historical ruler defaults. */
export function normalizeFormat(format?: MeasureFormatInput): NormalizedFormat {
    const leading = format?.leading ?? { value: 0.6, relative: true };
    return {
        font: format?.font || 'Archivo',
        baseFontSize: format?.baseFontSize ?? 10.5,
        entryHeadingSize: format?.entryHeadingSize ?? 11,
        leadingEm: leadingEm(leading.value, leading.relative, format?.baseFontSize ?? 10.5),
        marginLeft: format?.marginLeft ?? 15,
        marginRight: format?.marginRight ?? 15,
        pageSize: format?.pageSize || 'a4',
    };
}

function validateFormat(format: NormalizedFormat): void {
    for (const [field, value] of [
        ['baseFontSize', format.baseFontSize],
        ['entryHeadingSize', format.entryHeadingSize],
        ['leadingEm', format.leadingEm],
        ['marginLeft', format.marginLeft],
        ['marginRight', format.marginRight],
    ] as const) {
        if (!Number.isFinite(value)) throw fail(`${field} is not finite`);
    }
    if (format.baseFontSize <= 0 || format.entryHeadingSize <= 0) {
        throw fail('font sizes must be positive');
    }
    if (format.leadingEm <= 0) throw fail('resolved leading must be positive');
    if (format.marginLeft < 0 || format.marginRight < 0) throw fail('margins must not be negative');
}

function validateItems(items: MeasureItem[]): void {
    const seen = new Set<string>();
    for (const item of items) {
        if (!item.id) throw fail('item id is empty');
        if (seen.has(item.id)) throw fail(`duplicate item id ${item.id}`);
        seen.add(item.id);
        if (item.weight !== 'regular' && item.weight !== 'bold') {
            throw fail(`item ${item.id} has unknown weight ${JSON.stringify(item.weight)}`);
        }
        if (!Number.isFinite(item.fontSize) || item.fontSize <= 0) {
            throw fail(`item ${item.id} has no positive font size`);
        }
        if (!Number.isFinite(item.usableWidthPt) || item.usableWidthPt <= 0) {
            throw fail(`item ${item.id} has no positive width`);
        }
    }
}

/** Typst-WASM measurement client with result caching. */
export class CtypstMeasure {
    private readonly compiler: MeasureCompiler;
    private readonly manifestHash: string;
    private tag: string | undefined;
    private readonly results = new Map<string, CachedResult>();
    private calibrationValue: MeasureCalibration | undefined;
    private compiles = 0;

    /** Build around an initialized Typst compiler (worker keeps its own). */
    constructor(compiler: TypstCompiler) {
        this.compiler = compiler as unknown as MeasureCompiler;
        this.manifestHash = fnv1a64Hex(MANIFEST_JSON);
        this.compiler.addSource(PROGRAM_PATH, MEASURE_V1_TYP);
    }

    /**
     * Measure every item, compiling once per cache-miss batch. Results
     * follow request order. Empty input returns empty without compiling.
     */
    async measureAll(items: MeasureItem[], format?: MeasureFormatInput): Promise<MeasureResult[]> {
        const normalized = normalizeFormat(format);
        validateFormat(normalized);
        validateItems(items);
        if (items.length === 0) return [];
        const tag = this.formatTag(normalized);
        if (this.tag !== tag) {
            this.results.clear();
            this.calibrationValue = undefined;
            this.tag = tag;
        }
        const misses = items.filter((item) => !this.results.has(this.itemKey(normalized, item)));
        if (misses.length > 0) {
            await this.compileMisses(normalized, misses);
        }
        return items.map((item) => {
            const hit = this.results.get(this.itemKey(normalized, item));
            if (!hit) throw new Error(`${PROTOCOL_VERSION} cache omitted fragment ${item.id}`);
            const { key: _key, ...result } = hit;
            return result;
        });
    }

    /** Calibration ratios from the latest compile, if any ran. */
    calibration(): MeasureCalibration | undefined {
        return this.calibrationValue;
    }

    /** Engine compiles performed so far: one per miss batch, zero on hits. */
    compileCount(): number {
        return this.compiles;
    }

    private formatTag(format: NormalizedFormat): string {
        return `${PROTOCOL_VERSION}/${fnv1a64Hex(MEASURE_V1_TYP)}/${PACKAGE_VERSION}/${this.manifestHash}/${JSON.stringify(format)}`;
    }

    private itemKey(format: NormalizedFormat, item: MeasureItem): string {
        return `${this.formatTag(format)}/${JSON.stringify([item.id, item.text, item.fontSize, item.weight, item.usableWidthPt])}`;
    }

    private async compileMisses(format: NormalizedFormat, misses: MeasureItem[]): Promise<void> {
        const request = JSON.stringify({
            version: PROTOCOL_VERSION,
            format: {
                font: format.font,
                baseFontSize: format.baseFontSize,
                entryHeadingSize: format.entryHeadingSize,
                leadingEm: format.leadingEm,
                marginLeft: format.marginLeft,
                marginRight: format.marginRight,
                pageSize: format.pageSize,
            },
            items: misses.map(({ id, text, fontSize, weight, usableWidthPt }) => ({
                id,
                text,
                fontSize,
                weight,
                usableWidthPt,
            })),
        });
        this.compiler.addSource(REQUEST_PATH, request);
        const entries = await this.compiler.runWithWorld({ mainFilePath: PROGRAM_PATH }, async (world) => {
            const compiled = (await world.compile()) as { hasError?: boolean };
            if (compiled.hasError) throw new Error(`${PROTOCOL_VERSION} run failed to compile`);
            return world.query<Array<Record<string, unknown>>>({ selector: QUERY_SELECTOR, field: 'value' });
        });
        this.compiles += 1;
        this.decodeResponse(format, misses, entries);
    }

    private decodeResponse(
        format: NormalizedFormat,
        misses: MeasureItem[],
        entries: Array<Record<string, unknown>>,
    ): void {
        const rows = new Map<string, Record<string, unknown>>();
        let calibration: MeasureCalibration | undefined;
        for (const entry of entries) {
            if (typeof entry['id'] !== 'string') {
                throw new Error(`${PROTOCOL_VERSION} result has no id`);
            }
            const id = entry['id'] as string;
            if (id === '__calibration') {
                if (calibration !== undefined) throw new Error(`${PROTOCOL_VERSION} duplicated calibration`);
                calibration = decodeCalibration(entry);
                continue;
            }
            if (rows.has(id)) throw new Error(`${PROTOCOL_VERSION} duplicated fragment ${id}`);
            rows.set(id, entry);
        }
        if (calibration === undefined) throw new Error(`${PROTOCOL_VERSION} omitted calibration`);
        const wanted = new Set(misses.map((item) => item.id));
        for (const id of rows.keys()) {
            if (!wanted.has(id)) throw new Error(`${PROTOCOL_VERSION} returned unexpected fragment ${id}`);
        }
        for (const item of misses) {
            const row = rows.get(item.id);
            if (!row) throw new Error(`${PROTOCOL_VERSION} omitted fragment ${item.id}`);
            const raw = decodeRow(item.id, row);
            const result: CachedResult = {
                key: this.itemKey(format, item),
                id: item.id,
                widthPt: raw.w,
                heightPt: raw.h,
                lines: raw.lines,
                charBudget: charBudget(item.text, raw.w, item.usableWidthPt),
            };
            this.results.set(result.key, result);
        }
        this.calibrationValue = calibration;
    }
}

function decodeRow(id: string, row: Record<string, unknown>): { w: number; h: number; lines: number } {
    const w = row['w'];
    const h = row['h'];
    const lines = row['lines'];
    if (typeof w !== 'number' || !Number.isFinite(w) || w < 0) {
        throw new Error(`${PROTOCOL_VERSION} fragment ${id} has invalid width`);
    }
    if (typeof h !== 'number' || !Number.isFinite(h) || h < 0) {
        throw new Error(`${PROTOCOL_VERSION} fragment ${id} has invalid height`);
    }
    if (typeof lines !== 'number' || !Number.isInteger(lines) || lines < 1) {
        throw new Error(`${PROTOCOL_VERSION} fragment ${id} reports no lines`);
    }
    return { w, h, lines };
}

function decodeCalibration(entry: Record<string, unknown>): MeasureCalibration {
    const ratios = entry['ratios'] as Record<string, unknown> | undefined;
    const pick = (key: string): number => {
        const value = ratios?.[key];
        if (typeof value !== 'number' || !Number.isFinite(value)) {
            throw new Error(`${PROTOCOL_VERSION} calibration is malformed`);
        }
        return value;
    };
    return {
        capRatioRegular: pick('cap-reg'),
        advanceRatioRegular: pick('adv-reg'),
        capRatioBold: pick('cap-bold'),
        advanceRatioBold: pick('adv-bold'),
    };
}
