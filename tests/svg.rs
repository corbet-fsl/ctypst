//! Per-page SVG export contracts.

#[test]
fn svg_page_renders_vector_text() {
    let engine = ctypst::Engine::builder()
        .fonts(ctypst::fonts::documents())
        .source(
            "main.typ",
            "#set text(font: \"Archivo\", size: 11pt)\nHello *world*",
        )
        .expect("fixture installs")
        .build()
        .expect("engine builds");
    let output = engine
        .compile(ctypst::CompileRequest::new("main.typ".to_owned()))
        .expect("fixture compiles");
    let svg = engine
        .svg_page(&output.document, 0)
        .expect("first page renders");
    assert!(
        svg.starts_with("<svg"),
        "SVG document expected, got {}",
        &svg[..60.min(svg.len())]
    );
    assert!(
        svg.contains("<path"),
        "glyph outlines survive vector export"
    );
    assert!(
        engine.svg_page(&output.document, 1).is_err(),
        "missing page fails"
    );
}
