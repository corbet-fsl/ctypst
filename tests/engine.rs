//! End-to-end contracts for the embedded compiler and its capability boundary.

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use ctypst::{
    CompileRequest, DiagnosticsPolicy, Engine, Error, Limits, PageConstraint, fonts, query_json,
};
use tempfile::tempdir;

fn engine_with(source: &str) -> Engine {
    Engine::builder()
        .fonts(fonts::documents())
        .source("main.typ", source)
        .unwrap()
        .build()
        .unwrap()
}

#[test]
fn compiles_queries_and_updates_inputs_without_rebuilding_fonts() {
    let engine = engine_with(
        "#import sys: inputs\n#set text(font: \"Archivo\")\n#metadata(inputs.value) <probe>\n#inputs.value",
    );
    for value in ["first", "second"] {
        let output = engine
            .compile(
                CompileRequest::new("main.typ")
                    .input("value", value)
                    .pages(PageConstraint::Exactly(1)),
            )
            .unwrap();
        assert_eq!(query_json(&output.document, "probe").unwrap(), [value]);
    }
}

#[test]
fn request_scoped_virtual_source_can_change_one_compilation() {
    let engine = engine_with("#set text(font: \"Archivo\")\none");
    let first = engine.compile(CompileRequest::new("main.typ")).unwrap();
    let first_pdf = engine.pdf(&first.document, 0).unwrap();
    let second = engine
        .compile(
            CompileRequest::new("main.typ")
                .source_file("main.typ", "#set text(font: \"Archivo\")\ntwo"),
        )
        .unwrap();
    let second_pdf = engine.pdf(&second.document, 0).unwrap();
    assert_ne!(first_pdf, second_pdf);
    let third = engine.compile(CompileRequest::new("main.typ")).unwrap();
    assert_eq!(first_pdf, engine.pdf(&third.document, 0).unwrap());
}

#[test]
fn pdf_export_is_byte_reproducible() {
    let engine = engine_with("#set text(font: \"Archivo\")\nHello");
    let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
    assert_eq!(
        engine.pdf(&output.document, 0).unwrap(),
        engine.pdf(&output.document, 0).unwrap()
    );
}

#[test]
fn page_contract_is_enforced() {
    let engine = engine_with("#set text(font: \"Archivo\")\nHello");
    let error = engine
        .compile(CompileRequest::new("main.typ").pages(PageConstraint::Exactly(2)))
        .err()
        .unwrap();
    assert!(matches!(error, Error::PageCount { actual: 1, .. }));
}

#[test]
fn package_resolution_is_not_an_ambient_capability() {
    let engine = engine_with("#import \"@preview/not-real:0.1.0\": *");
    assert!(matches!(
        engine.compile(CompileRequest::new("main.typ")),
        Err(Error::Compile { .. })
    ));
}

#[test]
fn input_budget_is_enforced_before_compilation() {
    let limits = Limits {
        max_input_bytes: 4,
        ..Limits::default()
    };
    let engine = Engine::builder()
        .limits(limits)
        .fonts(fonts::documents())
        .source("main.typ", "#set text(font: \"Archivo\")\nHello")
        .unwrap()
        .build()
        .unwrap();
    let error = engine
        .compile(
            CompileRequest::new("main.typ")
                .inputs(BTreeMap::from([("key".to_owned(), "value".to_owned())]))
                .pages(PageConstraint::Any)
                .diagnostics(DiagnosticsPolicy::default()),
        )
        .err()
        .unwrap();
    assert!(matches!(error, Error::Limit { .. }));
}

#[test]
fn virtual_updates_and_compilation_are_atomic_between_threads() {
    let engine = Arc::new(engine_with(
        "#set text(font: \"Archivo\")\n#metadata(\"initial\") <probe>",
    ));
    let threads = ["alpha", "beta"].map(|value| {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            for _ in 0..8 {
                let source =
                    format!("#set text(font: \"Archivo\")\n#metadata(\"{value}\") <probe>");
                let output = engine
                    .compile(CompileRequest::new("main.typ").source_file("main.typ", source))
                    .unwrap();
                assert_eq!(query_json(&output.document, "probe").unwrap(), [value]);
            }
        })
    });
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn rejected_virtual_update_leaves_the_previous_snapshot_intact() {
    let limits = Limits {
        max_file_bytes: 80,
        ..Limits::default()
    };
    let engine = Engine::builder()
        .limits(limits)
        .fonts(fonts::documents())
        .source(
            "main.typ",
            "#set text(font: \"Archivo\")\n#metadata(\"kept\") <probe>",
        )
        .unwrap()
        .build()
        .unwrap();
    let rejected = "x".repeat(81);
    assert!(matches!(
        engine.compile(CompileRequest::new("main.typ").source_file("main.typ", rejected)),
        Err(Error::Limit { .. })
    ));
    let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
    assert_eq!(query_json(&output.document, "probe").unwrap(), ["kept"]);
}

#[test]
fn failed_compilation_rolls_back_request_scoped_files() {
    let engine = engine_with("#set text(font: \"Archivo\")\n#metadata(\"kept\") <probe>");
    assert!(matches!(
        engine.compile(
            CompileRequest::new("main.typ").source_file("main.typ", "#unknown-function()")
        ),
        Err(Error::Compile { .. })
    ));
    let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
    assert_eq!(query_json(&output.document, "probe").unwrap(), ["kept"]);
}

#[test]
fn malformed_font_is_rejected_even_beside_valid_fonts() {
    let error = Engine::builder()
        .fonts([fonts::archivo()[0], b"not a font"])
        .source("main.typ", "Hello")
        .unwrap()
        .build()
        .err()
        .unwrap();
    assert!(matches!(error, Error::InvalidFont { index: 1 }));
}

#[test]
fn raster_output_is_bounded_and_rgba() {
    let engine =
        engine_with("#set page(width: 20pt, height: 30pt)\n#set text(font: \"Archivo\")\nX");
    let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
    let page = engine.rasterize(&output.document, 0).unwrap();
    assert_eq!((page.width, page.height), (40, 60));
    assert_eq!(page.pixels.len(), 40 * 60 * 4);
}

#[test]
fn formatting_has_one_final_newline() {
    let formatted = ctypst::format_source("#let   x=1\n#x", 120).unwrap();
    assert!(formatted.ends_with('\n'));
    assert!(!formatted.ends_with("\n\n"));
}

#[cfg(unix)]
#[test]
fn filesystem_links_cannot_escape_the_canonical_root() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let root = directory.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("main.typ"), "#include \"escape.typ\"").unwrap();
    fs::write(directory.path().join("outside.typ"), "secret").unwrap();
    symlink(
        directory.path().join("outside.typ"),
        root.join("escape.typ"),
    )
    .unwrap();
    let engine = Engine::builder()
        .root(&root)
        .fonts(fonts::documents())
        .build()
        .unwrap();
    assert!(matches!(
        engine.compile(CompileRequest::new("main.typ")),
        Err(Error::Compile { .. })
    ));
}
