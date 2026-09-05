//! The checked-in font manifest must describe exactly the shipped bytes:
//! same file set, same SHA-256 per file. Consumers verify downloads and
//! cache keys against this manifest, so silent drift is a defect.

use std::collections::HashSet;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn fonts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts")
}

fn manifest_entries() -> Vec<(String, String)> {
    let manifest: serde_json::Value =
        serde_json::from_str(ctypst::fonts::MANIFEST).expect("font manifest is valid JSON");
    manifest["fonts"]
        .as_array()
        .expect("manifest lists fonts")
        .iter()
        .map(|entry| {
            (
                entry["file"]
                    .as_str()
                    .expect("font entry names a file")
                    .to_owned(),
                entry["sha256"]
                    .as_str()
                    .expect("font entry pins sha256")
                    .to_owned(),
            )
        })
        .collect()
}

#[test]
fn manifest_covers_every_shipped_font() {
    let dir = fonts_dir();
    let on_disk: HashSet<String> = std::fs::read_dir(&dir)
        .expect("fonts dir is readable")
        .map(|entry| {
            entry
                .expect("dir entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.to_ascii_lowercase().ends_with(".ttf"))
        .collect();
    let in_manifest: HashSet<String> = manifest_entries()
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    assert_eq!(
        on_disk, in_manifest,
        "manifest file set matches fonts/ bytes"
    );
}

#[test]
fn manifest_hashes_match_font_bytes() {
    for (file, sha256) in manifest_entries() {
        let bytes = std::fs::read(fonts_dir().join(&file)).expect("listed font is readable");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        assert_eq!(
            format!("{:x}", hasher.finalize()),
            sha256,
            "sha256 drifts for {file}"
        );
    }
}
