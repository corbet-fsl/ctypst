// ctypst-measure-v1: the one product-neutral Typst measurement program.
//
// The request arrives as DATA beside this program in the runtime VFS
// (`request.json`, addressed relatively so no filesystem root is needed).
// This program owns escaping, supported markup, calibration probes,
// natural and wrapped measurement, and line derivation. Consumers MUST NOT
// generate their own measurement program, reproduce calibration or line
// derivation, or define a parallel cache-key contract.
//
// Request shape (see `protocol/measure-v1/schema.json`):
//   version: "ctypst-measure-v1"
//   format: { font, baseFontSize, entryHeadingSize, leadingEm, marginLeft,
//             marginRight, pageSize }
//   items: [{ id, text, fontSize, weight, usableWidthPt }]
//
// Response: one `#metadata` per item plus one calibration record, all
// labelled `<ctypst-measure-v1>`. Each item reports natural width `w`,
// wrapped height `h` (both in points), and derived integer `lines`.
//
// Supported markup: `*strong*` and `_emphasis_` stay live; every other
// Typst-significant character (`\ [ ] # $ @ ` < ~`) is escaped and renders
// literally. An observable behavior change creates `ctypst-measure-v2`;
#let req = json("request.json")
#assert(req.version == "ctypst-measure-v1", message: "unsupported measure request version")

#let fmt = req.format
#let base-size = float(fmt.at("baseFontSize"))
#let head-size = float(fmt.at("entryHeadingSize"))
#let leading-em = float(fmt.at("leadingEm"))
#let font-name = str(fmt.at("font"))
#let page-width = if fmt.at("pageSize") == "us-letter" { 215.9mm } else { 210mm }
#let margin-left = float(fmt.at("marginLeft")) * 1mm
#let margin-right = float(fmt.at("marginRight")) * 1mm

#set page(width: page-width, margin: (top: 0pt, bottom: 0pt, left: margin-left, right: margin-right), height: auto)
#set text(font: font-name, size: base-size * 1pt, fill: black, top-edge: "cap-height", bottom-edge: "baseline")
#set par(leading: leading-em * 1em, justify: false, spacing: 0pt)
#set block(above: 0pt, below: 0pt)

// Escape exactly the historical ruler table; `*` and `_` stay live markup.
#let esc9(s) = {
  s.replace("\\", "\\\\").replace("[", "\\[").replace("]", "\\]")
   .replace("#", "\\#").replace("$", "\\$").replace("@", "\\@")
   .replace("`", "\\`").replace("<", "\\<").replace("~", "\\~")
}

#let styled(item) = {
  let weight = if item.weight == "bold" { "bold" } else { "regular" }
  text(size: float(item.at("fontSize")) * 1pt, weight: weight, eval(esc9(str(item.text)), mode: "markup", scope: (:)))
}

#let probe(size, weight, body) = {
  let m = measure(text(size: size * 1pt, weight: weight, body))
  (w: m.width.pt(), h: m.height.pt())
}

#context {
  let pb1 = probe(base-size, "regular", "X")
  let pb2 = probe(base-size, "regular", "X\nX")
  let ph1 = probe(head-size, "bold", "X")
  let ph2 = probe(head-size, "bold", "X\nX")
  let cal = (
    cap-reg: pb1.h / base-size,
    adv-reg: (pb2.h - pb1.h) / base-size,
    cap-bold: ph1.h / head-size,
    adv-bold: (ph2.h - ph1.h) / head-size,
  )
  let derive(h, size, weight) = {
    let c = if weight == "bold" { cal.at("cap-bold") } else { cal.at("cap-reg") }
    let a = if weight == "bold" { cal.at("adv-bold") } else { cal.at("adv-reg") }
    let first = c * size
    let adv = a * size
    if adv <= 0 { 1 } else { calc.max(1, int(calc.round((h - first + adv) / adv))) }
  }
  [#metadata((
    id: "__calibration",
    base-1: pb1,
    base-2: pb2,
    head-1: ph1,
    head-2: ph2,
    ratios: cal,
  )) <ctypst-measure-v1>]
  for item in req.items {
    let s = styled(item)
    let nat = measure(s)
    let wrap = measure(block(width: float(item.at("usableWidthPt")) * 1pt, s))
    let size = float(item.at("fontSize"))
    let weight = if item.weight == "bold" { "bold" } else { "regular" }
    [#metadata((
      id: str(item.id),
      w: nat.width.pt(),
      h: wrap.height.pt(),
      lines: derive(wrap.height.pt(), size, weight),
    )) <ctypst-measure-v1>]
  }
}
