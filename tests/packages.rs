//! Application-registered Typst packages resolve offline and nothing else does.

use std::fs;

use ctypst::{CompileRequest, Engine, EngineBuilder, Error, Limits, fonts, query_json};
use serde_json::json;
use tempfile::tempdir;

const SPEC: &str = "@preview/cgrade:0.1.3";
const MANIFEST: &str =
    "[package]\nname = \"cgrade\"\nversion = \"0.1.3\"\nentrypoint = \"typst/grade.typ\"\n";

fn grade_package() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("typst.toml", MANIFEST.into()),
        (
            "typst/grade.typ",
            b"#import \"generated/scale.typ\": steps\n\
              #let marker = read(\"/data/marker.txt\")\n\
              #let to-de(points) = steps.at(points)\n\
              #let from-de(grade) = steps.position(step => step == grade)\n\
              #let format-grade(grade) = marker + grade\n"
                .into(),
        ),
        (
            "typst/generated/scale.typ",
            b"#let steps = (\"6\", \"5\", \"4\", \"3\", \"2\", \"1\")\n".into(),
        ),
        ("data/marker.txt", b"Note ".into()),
    ]
}

fn builder(main: &str) -> EngineBuilder {
    Engine::builder()
        .fonts(fonts::documents())
        .source("main.typ", main)
        .unwrap()
}

fn engine(main: &str) -> Engine {
    builder(main)
        .package(SPEC, grade_package())
        .unwrap()
        .build()
        .unwrap()
}

fn compile_error(engine: &Engine) -> String {
    match engine.compile(CompileRequest::new("main.typ")) {
        Err(Error::Compile { diagnostics, .. }) => diagnostics,
        Err(error) => panic!("unexpected error: {error}"),
        Ok(_) => panic!("compilation unexpectedly succeeded"),
    }
}

fn registration_error(spec: &str, files: Vec<(&str, Vec<u8>)>) -> Error {
    builder("Hello").package(spec, files).err().unwrap()
}

fn with_manifest(manifest: &str) -> Vec<(&'static str, Vec<u8>)> {
    let mut files = grade_package();
    files[0].1 = manifest.into();
    files
}

#[test]
fn registered_package_imports_its_entrypoint_sources_and_assets() {
    let engine = engine(
        "#import \"@preview/cgrade:0.1.3\": to-de, from-de, format-grade\n\
         #set text(font: \"Archivo\")\n\
         #metadata((to-de(4), from-de(\"2\"), format-grade(\"1\"))) <probe>\n\
         #format-grade(to-de(5))",
    );
    let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
    assert_eq!(
        query_json(&output.document, "probe").unwrap(),
        [json!(["2", 4, "Note 1"])]
    );
}

#[test]
fn bare_package_import_binds_the_manifest_name() {
    let engine = engine(
        "#import \"@preview/cgrade:0.1.3\"\n\
         #set text(font: \"Archivo\")\n\
         #metadata(cgrade.to-de(0)) <probe>",
    );
    let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
    assert_eq!(query_json(&output.document, "probe").unwrap(), ["6"]);
}

#[test]
fn unregistered_packages_and_versions_are_rejected_by_spec() {
    for spec in [
        "@preview/other:0.1.0",
        "@preview/cgrade:0.1.2",
        "@local/cgrade:0.1.3",
    ] {
        let engine = engine(&format!("#import \"{spec}\": *"));
        let diagnostics = compile_error(&engine);
        assert!(
            diagnostics.contains(&format!("Typst package {spec} is not registered")),
            "{diagnostics}"
        );
    }
}

#[test]
fn unregistered_packages_are_rejected_beside_a_filesystem_root() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(
        root.join("main.typ"),
        "#import \"@preview/cgrade:0.1.3\": to-de\n#import \"@preview/other:0.1.0\": *",
    )
    .unwrap();
    let engine = Engine::builder()
        .root(&root)
        .fonts(fonts::documents())
        .package(SPEC, grade_package())
        .unwrap()
        .build()
        .unwrap();
    let report = engine.compile_tracked(CompileRequest::new("main.typ"));
    let Err(Error::Compile { diagnostics, .. }) = report.result else {
        panic!("the unregistered package must fail compilation");
    };
    assert!(
        diagnostics.contains("Typst package @preview/other:0.1.0 is not registered"),
        "{diagnostics}"
    );
    // Registered package files are in memory and never filesystem dependencies.
    assert_eq!(report.dependencies, [root.join("main.typ")]);
}

#[test]
fn packages_cannot_read_project_files_or_leave_their_root() {
    let cases = [
        ("/main.typ", "file not found"),
        // The entrypoint lives in `typst/`, so one parent is the package root.
        ("../main.typ", "file not found"),
        ("../../main.typ", "would escape the package root"),
    ];
    for (read, expected) in cases {
        let mut files = grade_package();
        files[1].1 = format!("#let leaked = read(\"{read}\")\n").into();
        let engine = builder("#import \"@preview/cgrade:0.1.3\": *")
            .package(SPEC, files)
            .unwrap()
            .build()
            .unwrap();
        let diagnostics = compile_error(&engine);
        assert!(diagnostics.contains(expected), "{read}: {diagnostics}");
    }
}

#[test]
fn package_paths_must_be_contained_and_normalized() {
    for path in [
        "../escape.typ",
        "typst/../../escape.typ",
        "/typst/grade.typ",
        "typst\\grade.typ",
        "./typst/grade.typ",
        "typst//grade.typ",
        "typst/",
        "",
    ] {
        let mut files = grade_package();
        files.push((path, b"x".into()));
        let error = registration_error(SPEC, files);
        assert!(
            matches!(&error, Error::InvalidVirtualPath(message) if message.contains("contained")),
            "{path:?}: {error}"
        );
    }
    let mut files = grade_package();
    files.push(("data/marker.txt", b"again".into()));
    assert!(matches!(
        registration_error(SPEC, files),
        Error::InvalidVirtualPath(message) if message.contains("supplied twice")
    ));
}

#[test]
fn package_specs_are_parsed_by_typst_and_registered_once() {
    for spec in [
        "cgrade:0.1.3",
        "@preview/cgrade",
        "@preview/cgrade:0.1",
        "@preview:0.1.3",
    ] {
        assert!(matches!(
            registration_error(spec, grade_package()),
            Error::InvalidVirtualPath(_)
        ));
    }
    let error = builder("Hello")
        .package(SPEC, grade_package())
        .unwrap()
        .package(SPEC, grade_package())
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::InvalidVirtualPath(message) if message.contains("already registered")
    ));
}

#[test]
fn package_manifest_must_describe_the_registered_spec() {
    let cases = [
        (
            "[package]\nname = \"other\"\nversion = \"0.1.3\"\nentrypoint = \"typst/grade.typ\"\n",
            "mismatched name",
        ),
        (
            "[package]\nname = \"cgrade\"\nversion = \"0.1.2\"\nentrypoint = \"typst/grade.typ\"\n",
            "mismatched version",
        ),
        (
            "[package]\nname = \"cgrade\"\nversion = \"0.1.3\"\nentrypoint = \"typst/missing.typ\"\n",
            "entrypoint",
        ),
        ("[package]\nname = \"cgrade\"\n", "malformed"),
        ("not toml", "malformed"),
    ];
    for (manifest, expected) in cases {
        let error = registration_error(SPEC, with_manifest(manifest));
        assert!(
            matches!(&error, Error::InvalidVirtualPath(message) if message.contains(expected)),
            "{error}"
        );
    }
    let mut files = grade_package();
    files.remove(0);
    assert!(matches!(
        registration_error(SPEC, files),
        Error::InvalidVirtualPath(message) if message.contains("manifest is missing")
    ));
}

#[test]
fn package_limits_apply_at_registration_and_build() {
    let small_files = Limits {
        max_file_bytes: 64,
        ..Limits::default()
    };
    let error = builder("Hello")
        .limits(small_files)
        .package(SPEC, grade_package())
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::Limit {
            limit_name: "file bytes",
            ..
        }
    ));
    // Limits lowered after registration are enforced when the engine is built.
    let short_paths = Limits {
        max_path_bytes: 16,
        ..Limits::default()
    };
    let error = builder("Hello")
        .package(SPEC, grade_package())
        .unwrap()
        .limits(short_paths)
        .build()
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::Limit {
            limit_name: "path bytes",
            ..
        }
    ));
    let package_bytes = grade_package()
        .iter()
        .map(|(_, data)| data.len())
        .sum::<usize>();
    let one_package = Limits {
        max_total_bytes: package_bytes,
        ..Limits::default()
    };
    let other = with_manifest(
        "[package]\nname = \"cgrade\"\nversion = \"0.1.4\"\nentrypoint = \"typst/grade.typ\"\n",
    );
    let error = builder("Hello")
        .limits(one_package)
        .package(SPEC, grade_package())
        .unwrap()
        .package("@preview/cgrade:0.1.4", other)
        .unwrap()
        .build()
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::Limit {
            limit_name: "package data bytes",
            ..
        }
    ));
}

#[test]
fn package_registration_order_does_not_change_pdf_bytes() {
    let main = "#import \"@preview/cgrade:0.1.3\": to-de, format-grade\n\
                #set text(font: \"Archivo\")\n\
                #format-grade(to-de(3))";
    let mut reversed = grade_package();
    reversed.reverse();
    let pdfs = [grade_package(), reversed].map(|files| {
        let engine = builder(main).package(SPEC, files).unwrap().build().unwrap();
        let output = engine.compile(CompileRequest::new("main.typ")).unwrap();
        engine.pdf(&output.document, 0).unwrap()
    });
    assert_eq!(pdfs[0], pdfs[1]);
}
