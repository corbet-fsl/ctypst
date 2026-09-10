import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const require = createRequire(import.meta.url);
const { Ctypst } = require(join(process.argv[2], 'wasm/nodejs/ctypst.js'));
const compiler = new Ctypst();
const vectors = JSON.parse(readFileSync(process.argv[3], 'utf8'));
let items = 0;
for (const { request } of vectors.requests) {
  const f = request.format;
  const format = JSON.stringify({ font: f.font, baseFontSize: f.baseFontSize,
    entryHeadingSize: f.entryHeadingSize, leadingValue: f.leadingEm,
    leadingRelative: true, marginLeft: f.marginLeft,
    marginRight: f.marginRight, pageSize: f.pageSize });
  const result = JSON.parse(compiler.measure_all(format, JSON.stringify(request.items)));
  if (result.results.length !== request.items.length) throw new Error('Measurement count differs');
  items += result.results.length;
}
const document = compiler.compile('#set text(font: "Archivo")\nsmoke', '{}');
if (document.page_count() !== 1) throw new Error('Page count differs');
if (!document.svg_page(0).startsWith('<svg')) throw new Error('SVG export failed');
if (Buffer.from(document.pdf().slice(0, 5)).toString() !== '%PDF-') throw new Error('PDF export failed');
console.log(`Packaged WASM smoke passed: ${items} measured items, SVG and PDF`);
