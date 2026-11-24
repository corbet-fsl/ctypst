// SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Offline failure fixtures for the irreversible publication boundary.

use ctypst_release::*;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn files(entries: &[(&str, &[u8])]) -> Files {
    entries
        .iter()
        .map(|(n, d)| (n.to_string(), d.to_vec()))
        .collect()
}

fn header(name: &[u8], size: u64, kind: tar::EntryType, mode: u32) -> tar::Header {
    let mut h = tar::Header::new_ustar();
    h.as_old_mut().name[..name.len()].copy_from_slice(name);
    h.set_size(size);
    h.set_mode(mode);
    h.set_entry_type(kind);
    h.set_cksum();
    h
}

/// A tar of `entries`, with a git-style pax global header naming `commit` when given.
fn make_tar(entries: &Files, commit: Option<&str>, mode: u32) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    if let Some(commit) = commit {
        let record = format!("52 comment={commit}\n");
        assert_eq!(record.len(), 52);
        let h = header(
            b"pax_global_header",
            52,
            tar::EntryType::XGlobalHeader,
            0o666,
        );
        builder.append(&h, record.as_bytes()).unwrap();
    }
    for (name, data) in entries {
        let h = header(
            name.as_bytes(),
            data.len() as u64,
            tar::EntryType::Regular,
            mode,
        );
        builder.append(&h, &data[..]).unwrap();
    }
    builder.into_inner().unwrap()
}

fn fixture_files(version: &str) -> Files {
    let package = format!("{{\"version\":\"{version}\"}}");
    files(&[
        (
            "Cargo.toml",
            format!("[package]\nname=\"ctypst\"\nversion=\"{version}\"\n").as_bytes(),
        ),
        ("Cargo.lock", b"lock\n"),
        ("src/lib.rs", b"original"),
        ("js/ctypst/package.json", package.as_bytes()),
        ("js/ctypst/jsr.json", package.as_bytes()),
    ])
}

fn env(pairs: &[(&str, String)]) -> Env {
    let mut e = Env::default();
    for (k, v) in pairs {
        e.0.insert(k.to_string(), v.clone());
    }
    e
}

#[test]
fn modified_and_added_source_inputs_are_rejected() {
    for change in ["modified", "added", "commit", "control"] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("source");
        let source = fixture_files("0.3.2");
        unpack(&source, &root).unwrap();
        let archive_commit = if change == "commit" {
            "b".repeat(40)
        } else {
            "a".repeat(40)
        };
        let archive = make_tar(&source, Some(&archive_commit), 0o644);
        let archive_path = temporary.path().join("source.tar");
        fs::write(&archive_path, &archive).unwrap();
        match change {
            "modified" => fs::write(root.join("src/lib.rs"), b"modified").unwrap(),
            "added" => fs::write(root.join("src/additional.rs"), b"added").unwrap(),
            _ => {}
        }
        let environment = env(&[
            ("CI_COMMIT_SHA", "a".repeat(40)),
            (
                "SOURCE_ARCHIVE",
                archive_path.to_string_lossy().into_owned(),
            ),
            ("SOURCE_SHA256", digest(&archive)),
            ("RELEASE_TAG", "v0.3.2".into()),
        ]);
        let result = source_identity(&root, &environment, false);
        if change == "control" {
            // The untouched fixture is accepted, so the rejections above are the guards at work.
            assert!(result.is_ok(), "{:?}", result.err());
        } else {
            assert!(matches!(result, Err(Fail::Msg(_))), "{change}: {result:?}");
        }
    }
}

#[test]
fn canonical_source_identity_ignores_tar_encoding_but_binds_modes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = files(&[("script.sh", b"#!/bin/sh\nexit 0\n")]);
    unpack(&source, root).unwrap();
    let plain = make_tar(&source, None, 0o644);
    let with_header = make_tar(&source, Some(&"a".repeat(40)), 0o644);
    assert_ne!(digest(&plain), digest(&with_header));
    assert_eq!(
        source_content_identity(root, &plain, &source).unwrap(),
        source_content_identity(root, &with_header, &source).unwrap()
    );
    fs::set_permissions(root.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let error = source_content_identity(root, &plain, &source).unwrap_err();
    assert!(error.to_string().contains("executable mode differs"));
}

#[test]
fn traversal_links_and_duplicates_are_refused() {
    for kind in ["traversal", "symlink", "duplicate"] {
        let mut builder = tar::Builder::new(Vec::new());
        let name: &[u8] = if kind == "traversal" {
            b"../outside"
        } else {
            b"package/file"
        };
        if kind == "symlink" {
            let mut h = header(name, 0, tar::EntryType::Symlink, 0o644);
            h.set_link_name("/outside").unwrap();
            h.set_cksum();
            builder.append(&h, &b""[..]).unwrap();
        } else {
            builder
                .append(&header(name, 1, tar::EntryType::Regular, 0o644), &b"x"[..])
                .unwrap();
            if kind == "duplicate" {
                builder
                    .append(&header(name, 1, tar::EntryType::Regular, 0o644), &b"y"[..])
                    .unwrap();
            }
        }
        let data = builder.into_inner().unwrap();
        assert!(
            matches!(archive_files(&data, "package/"), Err(Fail::Msg(_))),
            "{kind}"
        );
    }
}

#[test]
fn actual_driver_refuses_credentials_before_any_build() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let root = directory.join("source");
    let marker = directory.join("build-was-invoked");
    let commit = "a".repeat(40);
    let mut source = fixture_files("0.3.2");
    source.insert(
        "scripts/ci.sh".into(),
        b"#!/bin/sh\nprintf called > \"$FIXTURE_BUILD_MARKER\"\nexit 99\n".to_vec(),
    );
    unpack(&source, &root).unwrap();
    let archive = make_tar(&source, Some(&commit), 0o644);
    let archive_path = directory.join("source.tar");
    fs::write(&archive_path, &archive).unwrap();
    for name in TOKENS {
        let secret = "fixture-secret-that-must-not-be-printed";
        let mut command = Command::new(env!("CARGO_BIN_EXE_ctypst-release"));
        command
            .args(["prepare", "cargo"])
            .current_dir(&root)
            .env_clear();
        for (key, value) in std::env::vars().filter(|(k, _)| !TOKENS.contains(&k.as_str())) {
            command.env(key, value);
        }
        command
            .env("CI_COMMIT_SHA", &commit)
            .env("SOURCE_ARCHIVE", &archive_path)
            .env("SOURCE_SHA256", digest(&archive))
            .env("RELEASE_TAG", "v0.3.2")
            .env("RELEASE_ARTIFACT_DIR", directory.join("artifacts"))
            .env("FIXTURE_BUILD_MARKER", &marker)
            .env(name, secret);
        let result = command.output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.status.success(), "{name}: {text}");
        assert!(
            text.contains("Publication credentials must be absent from preparation"),
            "{name}: {text}"
        );
        assert!(!text.contains(secret), "{name}");
        assert!(
            !marker.exists(),
            "A build started with publication credentials present ({name})"
        );
    }
}

const ESC: &str = "\\u00e9\\ud83d\\ude00";

#[test]
fn json_matches_python_shapes() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"b":[1,"é😀"],"a":{},"c":[]}"#).unwrap();
    assert_eq!(
        dump(&value, Style::Default),
        r#"{"b": [1, "@"], "a": {}, "c": []}"#.replace("@", ESC)
    );
    assert_eq!(
        dump(&value, Style::Compact),
        r#"{"b":[1,"@"],"a":{},"c":[]}"#.replace("@", ESC)
    );
    assert_eq!(
        dump(&value, Style::Pretty),
        "{\n  \"a\": {},\n  \"b\": [\n    1,\n    \"@\"\n  ],\n  \"c\": []\n}".replace("@", ESC)
    );
}

#[test]
fn versions_gate_detects_mismatch() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let mut source = fixture_files("1.2.3");
    source.insert(
        "bindings/python/Cargo.toml".into(),
        b"[package]\nversion=\"1.2.3\"\n".to_vec(),
    );
    source.insert(
        "bindings/python/pyproject.toml".into(),
        b"[project]\nversion=\"1.2.3\"\n".to_vec(),
    );
    unpack(&source, root).unwrap();
    assert_eq!(
        check_versions(root).unwrap(),
        "All package versions match 1.2.3"
    );
    fs::write(
        root.join("bindings/python/pyproject.toml"),
        "[project]\nversion=\"1.2.4\"\n",
    )
    .unwrap();
    let error = check_versions(root).unwrap_err().to_string();
    assert_eq!(
        error,
        "bindings/python/pyproject.toml does not match crate version 1.2.3"
    );
}

#[test]
fn dependencies_are_normalized_and_unresolved_ones_refused() {
    let manifest: toml::Value = toml::from_str("[dependencies]\nz = \"1\"\na = { version = \"2\", package = \"b\", optional = true }\n[target.'cfg(unix)'.dev-dependencies]\nq = \"3\"\n")
        .unwrap();
    let rows = dependencies(&manifest).unwrap();
    let names: Vec<_> = rows
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["b", "z", "q"]);
    assert_eq!(rows[0]["explicit_name_in_toml"], "a");
    let local: toml::Value =
        toml::from_str("[dependencies]\nx = { path = \"../x\", version = \"1\" }\n").unwrap();
    assert!(matches!(dependencies(&local), Err(Fail::Msg(_))));
    let _ = BTreeMap::<u8, u8>::new();
    let _ = Path::new("");
}
