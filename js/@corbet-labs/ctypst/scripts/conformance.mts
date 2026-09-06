/**
 * WASM side of the cross-runtime conformance gate: runs every
 * `protocol/measure-v1` vector through the Typst-WASM adapter and compares
 * with `expected.json` under the documented envelope (ids/lines exact,
 * floats within one ulp). Also pins end-to-end character budgets and the
 * compile-batching contract. Exits non-zero with the first mismatch.
 *
 * Run from the package root after `scripts/sync-assets.sh`:
 *   bun ./scripts/conformance.mts
 */
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { createTypstCompiler, loadFonts } from '@myriaddreamin/typst.ts';
import {
    CtypstMeasure,
    type MeasureFormatInput,
    type MeasureItem,
} from '../src/index.ts';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

function ulpDistance(a: number, b: number): bigint {
    const view = new DataView(new ArrayBuffer(8));
    view.setFloat64(0, a);
    const ai = view.getBigInt64(0);
    view.setFloat64(0, b);
    const bi = view.getBigInt64(0);
    return ai >= bi ? ai - bi : bi - ai;
}

function checkUlp(actual: number, expected: number, what: string): void {
    if (ulpDistance(actual, expected) > 1n) {
        throw new Error(`${what}: ${actual} vs ${expected} exceeds one ulp`);
    }
}

function checkEqual<T>(actual: T, expected: T, what: string): void {
    if (actual !== expected) {
        throw new Error(`${what}: ${JSON.stringify(actual)} vs ${JSON.stringify(expected)}`);
    }
}

const vectors = JSON.parse(
    readFileSync(join(ROOT, 'protocol/measure-v1/requests.json'), 'utf8'),
) as {
    requests: Array<{ name: string; request: { format: Record<string, number | string>; items: MeasureItem[] } }>;
};
const expected = JSON.parse(readFileSync(join(ROOT, 'protocol/measure-v1/expected.json'), 'utf8')) as {
    expected: Array<{
        name: string;
        results: Array<{ id: string; w: number; h: number; lines: number }>;
        calibration: { ratios: Record<string, number> };
    }>;
};
const wantByName = new Map(expected.expected.map((entry) => [entry.name, entry]));

const fontDir = join(ROOT, 'fonts');
const fontBytes = readdirSync(fontDir)
    .filter((file: string) => file.endsWith('.ttf'))
    .sort()
    .map((file: string) => new Uint8Array(readFileSync(join(fontDir, file))));

const compiler = createTypstCompiler();
await compiler.init({ beforeBuild: [loadFonts(fontBytes, { assets: false })] });
const measure = new CtypstMeasure(compiler);

for (const { name, request } of vectors.requests) {
    const format = request.format as unknown as MeasureFormatInput & {
        baseFontSize: number;
        entryHeadingSize: number;
    };
    const results = await measure.measureAll(request.items, {
        font: request.format['font'] as string,
        baseFontSize: format.baseFontSize,
        entryHeadingSize: format.entryHeadingSize,
        leading: { value: request.format['leadingEm'] as number, relative: true },
        marginLeft: request.format['marginLeft'] as number,
        marginRight: request.format['marginRight'] as number,
        pageSize: request.format['pageSize'] as string,
    });
    const want = wantByName.get(name);
    if (!want) throw new Error(`no expected vector for ${name}`);
    checkEqual(results.length, want.results.length, `${name} result count`);
    for (let index = 0; index < results.length; index++) {
        const got = results[index];
        const wantRow = want.results[index];
        checkEqual(got.id, wantRow.id, `${name} id order`);
        checkUlp(got.widthPt, wantRow.w, `${name}/${got.id} width`);
        checkUlp(got.heightPt, wantRow.h, `${name}/${got.id} height`);
        checkEqual(got.lines, wantRow.lines, `${name}/${got.id} lines`);
    }
    const calibration = measure.calibration();
    if (!calibration) throw new Error(`${name} omitted calibration`);
    checkUlp(calibration.capRatioRegular, want.calibration.ratios['cap-reg'] as number, `${name} cap-reg`);
    checkUlp(calibration.advanceRatioRegular, want.calibration.ratios['adv-reg'] as number, `${name} adv-reg`);
    checkUlp(calibration.capRatioBold, want.calibration.ratios['cap-bold'] as number, `${name} cap-bold`);
    checkUlp(calibration.advanceRatioBold, want.calibration.ratios['adv-bold'] as number, `${name} adv-bold`);
    console.log(`vector ${name}: ${results.length} items identical`);
}

// End-to-end budgets through the adapter (frozen formula).
const budgets = await measure.measureAll(
    [
        { id: 'e', text: '', fontSize: 10.5, weight: 'regular', usableWidthPt: 400 },
        { id: 'u', text: '😀 done', fontSize: 10.5, weight: 'regular', usableWidthPt: 400 },
    ],
    { font: 'Archivo', baseFontSize: 9.5, entryHeadingSize: 11 },
);
const emptyBudget = budgets.find((result) => result.id === 'e');
const emojiBudget = budgets.find((result) => result.id === 'u');
if (!emptyBudget || !emojiBudget) throw new Error('budget probe ids missing');
checkEqual(emptyBudget.charBudget, null, 'empty budget');
checkEqual(emojiBudget.charBudget, 78, 'emoji budget');

// Batching contract: r1 + r2 (format change purges) + budgets = 3 compiles.
checkEqual(measure.compileCount(), 3, 'compile batches');
const empty = await measure.measureAll([], { font: 'Archivo' });
checkEqual(empty.length, 0, 'empty measures empty');
checkEqual(measure.compileCount(), 3, 'empty never compiles');

console.log(`WASM conformance green: ${vectors.requests.length} vectors (ids/lines exact, floats within one ulp)`);
