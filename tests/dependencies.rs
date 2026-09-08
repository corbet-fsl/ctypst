//! Filesystem observation must survive incremental compilation and failures.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ctypst::{CompileReport, CompileRequest, Engine, fonts, query_json};
use tempfile::tempdir;

fn engine(root: &Path) -> Engine {
    Engine::builder()
        .root(root)
        .fonts(fonts::documents())
        .build()
        .unwrap()
}

fn dependencies(report: &CompileReport) -> BTreeSet<PathBuf> {
    report.dependencies.iter().cloned().collect()
}

#[test]
fn transitive_reads_are_complete_after_cache_reuse_and_change_with_inputs() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::create_dir(root.join("parts")).unwrap();
    fs::write(
        root.join("main.typ"),
        "#set text(font: \"Archivo\")\n#include \"parts/section.typ\"",
    )
    .unwrap();
    fs::write(
        root.join("parts/section.typ"),
        "#let value = read(\"/\" + sys.inputs.choice)\n#metadata(value) <value>\n#value",
    )
    .unwrap();
    fs::write(root.join("wording.md"), "First").unwrap();
    fs::write(root.join("other-data"), "Second").unwrap();
    let compiler = engine(&root);
    let expected = BTreeSet::from([
        root.join("main.typ"),
        root.join("parts/section.typ"),
        root.join("wording.md"),
    ]);
    for _ in 0..3 {
        let report =
            compiler.compile_tracked(CompileRequest::new("main.typ").input("choice", "wording.md"));
        assert_eq!(dependencies(&report), expected);
        assert_eq!(
            query_json(&report.result.unwrap().document, "value").unwrap(),
            ["First"]
        );
    }
    fs::write(root.join("wording.md"), "Updated").unwrap();
    let report =
        compiler.compile_tracked(CompileRequest::new("main.typ").input("choice", "wording.md"));
    assert_eq!(dependencies(&report), expected);
    assert_eq!(
        query_json(&report.result.unwrap().document, "value").unwrap(),
        ["Updated"]
    );
    let report =
        compiler.compile_tracked(CompileRequest::new("main.typ").input("choice", "other-data"));
    assert_eq!(
        dependencies(&report),
        BTreeSet::from([
            root.join("main.typ"),
            root.join("parts/section.typ"),
            root.join("other-data")
        ])
    );
    assert_eq!(
        query_json(&report.result.unwrap().document, "value").unwrap(),
        ["Second"]
    );
}

#[test]
fn missing_and_deleted_files_remain_dependencies_on_failure() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(
        root.join("main.typ"),
        "#set text(font: \"Archivo\")\n#read(\"pending/data.csv\")",
    )
    .unwrap();
    let compiler = engine(&root);
    let expected = BTreeSet::from([root.join("main.typ"), root.join("pending/data.csv")]);
    for _ in 0..3 {
        let report = compiler.compile_tracked(CompileRequest::new("main.typ"));
        assert!(report.result.is_err());
        assert_eq!(dependencies(&report), expected);
    }
    fs::create_dir(root.join("pending")).unwrap();
    fs::write(root.join("pending/data.csv"), "Now present").unwrap();
    assert!(
        compiler
            .compile_tracked(CompileRequest::new("main.typ"))
            .result
            .is_ok()
    );
    fs::remove_file(root.join("pending/data.csv")).unwrap();
    let report = compiler.compile_tracked(CompileRequest::new("main.typ"));
    assert!(report.result.is_err());
    assert_eq!(dependencies(&report), expected);
}

#[test]
fn virtual_files_do_not_watch_shadowed_disk_files() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("main.typ"), "invalid #code(").unwrap();
    let compiler = engine(directory.path());
    let report = compiler.compile_tracked(
        CompileRequest::new("main.typ")
            .source_file(
                "main.typ",
                "#set text(font: \"Archivo\")\n#read(\"data.md\")",
            )
            .binary_file("data.md", b"Virtual".to_vec()),
    );
    assert!(report.result.is_ok());
    assert!(report.dependencies.is_empty());
}

#[test]
fn concurrent_reports_belong_to_their_own_request() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for name in ["a.typ", "b.typ"] {
        fs::write(root.join(name), "#set text(font: \"Archivo\")\nHello").unwrap();
    }
    let compiler = Arc::new(engine(&root));
    let threads: Vec<_> = ["a.typ", "b.typ"]
        .into_iter()
        .map(|name| {
            let compiler = compiler.clone();
            let expected = root.join(name);
            std::thread::spawn(move || {
                let report = compiler.compile_tracked(CompileRequest::new(name));
                assert!(report.result.is_ok());
                assert_eq!(report.dependencies, [expected]);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn symlink_names_and_targets_are_observed_without_exposing_external_targets() {
    use std::os::unix::fs::symlink;
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(
        root.join("main.typ"),
        "#set text(font: \"Archivo\")\n#read(\"link.md\")",
    )
    .unwrap();
    fs::write(root.join("first.md"), "First").unwrap();
    fs::write(root.join("second.md"), "Second").unwrap();
    symlink("first.md", root.join("link.md")).unwrap();
    let compiler = engine(&root);
    let report = compiler.compile_tracked(CompileRequest::new("main.typ"));
    assert!(report.result.is_ok());
    assert_eq!(
        dependencies(&report),
        BTreeSet::from([
            root.join("main.typ"),
            root.join("link.md"),
            root.join("first.md")
        ])
    );
    fs::remove_file(root.join("link.md")).unwrap();
    symlink("second.md", root.join("link.md")).unwrap();
    let report = compiler.compile_tracked(CompileRequest::new("main.typ"));
    assert!(report.result.is_ok());
    assert_eq!(
        dependencies(&report),
        BTreeSet::from([
            root.join("main.typ"),
            root.join("link.md"),
            root.join("second.md")
        ])
    );
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("private.md"), "Must not be read").unwrap();
    fs::remove_file(root.join("link.md")).unwrap();
    symlink(outside.path().join("private.md"), root.join("link.md")).unwrap();
    let report = compiler.compile_tracked(CompileRequest::new("main.typ"));
    assert!(report.result.is_err());
    assert_eq!(
        dependencies(&report),
        BTreeSet::from([root.join("main.typ"), root.join("link.md")])
    );
}
