// SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Prepare the immutable ctypst packages of a release (the crate and the Wasm npm package).
//!
//! The release run (`ccid release build`, .ci/release-build.sh) calls this; publication is the
//! platform publisher's, from the release's files. This command never installs tools.
//! `versions` is the package-version consistency gate of scripts/ci.sh.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const REPOSITORY: &str = "corbet-libs/ctypst";
pub const TOKENS: [&str; 5] = [
    "CARGO_REGISTRY_TOKEN",
    "NPM_TOKEN",
    "NODE_AUTH_TOKEN",
    "JSR_TOKEN",
    "GH_TOKEN",
];

/// A failure. `Msg` is a deliberate, safe-to-print message; `Other` prints only the error kind so
/// that tokens in child diagnostics or arguments cannot leak.
#[derive(Debug)]
pub enum Fail {
    Msg(String),
    Other(&'static str),
    Usage(String),
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Msg(m) | Fail::Usage(m) => write!(f, "{m}"),
            Fail::Other(kind) => write!(
                f,
                "Release failed ({kind}); retained artifacts and journals were not removed"
            ),
        }
    }
}

impl From<std::io::Error> for Fail {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        Fail::Other(match error.kind() {
            NotFound => "FileNotFoundError",
            WouldBlock => "BlockingIOError",
            PermissionDenied => "PermissionError",
            AlreadyExists => "FileExistsError",
            _ => "OSError",
        })
    }
}
impl From<serde_json::Error> for Fail {
    fn from(_: serde_json::Error) -> Self {
        Fail::Other("JSONDecodeError")
    }
}
impl From<toml::de::Error> for Fail {
    fn from(_: toml::de::Error) -> Self {
        Fail::Other("TOMLDecodeError")
    }
}
impl From<std::string::FromUtf8Error> for Fail {
    fn from(_: std::string::FromUtf8Error) -> Self {
        Fail::Other("UnicodeDecodeError")
    }
}

pub type R<T> = Result<T, Fail>;

pub fn require(condition: bool, message: &str) -> R<()> {
    if condition {
        Ok(())
    } else {
        Err(Fail::Msg(message.to_string()))
    }
}

fn key_error<T>() -> R<T> {
    Err(Fail::Other("KeyError"))
}

/// The process environment, injectable for tests.
#[derive(Clone, Debug, Default)]
pub struct Env(pub BTreeMap<String, String>);

impl Env {
    pub fn from_process() -> Env {
        Env(std::env::vars().collect())
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
    /// Python's `os.environ.get(name)` truthiness: set and non-empty.
    pub fn nonempty(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|v| !v.is_empty())
    }
    pub fn required(&self, name: &str) -> R<&str> {
        match self.get(name) {
            Some(v) => Ok(v),
            None => key_error(),
        }
    }
}

pub fn digest(data: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(data) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ---------------------------------------------------------------- JSON (Python-compatible)

fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x80 => out.push(c),
            c => {
                let mut buffer = [0u16; 2];
                for unit in c.encode_utf16(&mut buffer) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// Output shape of Python's `json.dumps`.
#[derive(Clone, Copy)]
pub enum Style {
    /// `json.dumps(value)`: separators ", " and ": ", insertion order.
    Default,
    /// `json.dumps(value, separators=(",", ":"))`.
    Compact,
    /// `json.dump(value, indent=2, sort_keys=True)`.
    Pretty,
}

pub fn dump(value: &Value, style: Style) -> String {
    let mut out = String::new();
    write_value(value, style, 0, &mut out);
    out
}

fn write_value(value: &Value, style: Style, depth: usize, out: &mut String) {
    let (item_sep, key_sep) = match style {
        Style::Default => (", ", ": "),
        Style::Compact => (",", ":"),
        Style::Pretty => (",", ": "),
    };
    let pretty = matches!(style, Style::Pretty);
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(item_sep);
                }
                if pretty {
                    newline(out, depth + 1);
                }
                write_value(item, style, depth + 1, out);
            }
            if pretty {
                newline(out, depth);
            }
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if pretty {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            out.push('{');
            for (index, (name, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(item_sep);
                }
                if pretty {
                    newline(out, depth + 1);
                }
                escape(name, out);
                out.push_str(key_sep);
                write_value(item, style, depth + 1, out);
            }
            if pretty {
                newline(out, depth);
            }
            out.push('}');
        }
        Value::Array(_) => out.push_str("[]"),
        Value::Object(_) => out.push_str("{}"),
        Value::String(s) => escape(s, out),
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
    }
}

pub fn read_json(path: &Path) -> R<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

pub fn write_json(path: &Path, value: &Value) -> R<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    {
        let mut stream = fs::File::create(&temporary)?;
        stream.write_all(dump(value, Style::Pretty).as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        stream.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

// ---------------------------------------------------------------- processes

/// Run a command; with `capture`, return stdout and stderr merged and stripped.
pub fn run(command: &[&str], cwd: &Path, env: Option<&Env>, capture: bool) -> R<Option<String>> {
    let mut child = Command::new(command[0]);
    child.args(&command[1..]).current_dir(cwd);
    if let Some(env) = env {
        child.env_clear().envs(&env.0);
    }
    let mut reader = None;
    if capture {
        let (r, w) = std::io::pipe()?;
        child.stdout(w.try_clone()?).stderr(w);
        reader = Some(r);
    }
    let mut handle = child.spawn()?;
    // Drop our write ends (held inside `child`) before reading.
    drop(child);
    let mut output = Vec::new();
    if let Some(mut r) = reader {
        r.read_to_end(&mut output)?;
    }
    let status = handle.wait()?;
    let name = Path::new(command[0])
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let code = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0));
    require(
        status.success(),
        &format!("{name} failed (exit {code}); no publication retry was attempted"),
    )?;
    if capture {
        Ok(Some(String::from_utf8_lossy(&output).trim().to_string()))
    } else {
        Ok(None)
    }
}

fn run_capture(command: &[&str], cwd: &Path) -> R<String> {
    Ok(run(command, cwd, None, true)?.unwrap_or_default())
}

fn check_output(command: &[&str], cwd: &Path) -> R<Vec<u8>> {
    let output = Command::new(command[0])
        .args(&command[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        return Err(Fail::Other("CalledProcessError"));
    }
    Ok(output.stdout)
}

// ---------------------------------------------------------------- archives

fn is_hex40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_version(s: &str) -> bool {
    let (core, pre) = match s.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

/// `git get-tar-commit-id`: the commit recorded in the pax global header of a `git archive` tar.
pub fn tar_commit_id(data: &[u8]) -> R<String> {
    let plain;
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        let mut buffer = Vec::new();
        flate2::read::GzDecoder::new(data).read_to_end(&mut buffer)?;
        plain = buffer;
        &plain[..]
    } else {
        data
    };
    let fail = || Fail::Other("CalledProcessError");
    if data.len() < 512 + 52 || data[156] != b'g' {
        return Err(fail());
    }
    let size = std::str::from_utf8(&data[124..136])
        .ok()
        .and_then(|s| u64::from_str_radix(s.trim_matches(|c| c == '\0' || c == ' '), 8).ok());
    if size != Some(52) {
        return Err(fail());
    }
    let record = &data[512..512 + 52];
    let text = std::str::from_utf8(record).map_err(|_| fail())?;
    let id = text
        .strip_prefix("52 comment=")
        .and_then(|rest| rest.strip_suffix('\n'))
        .ok_or_else(fail)?;
    if id.len() != 40 {
        return Err(fail());
    }
    Ok(id.to_string())
}

fn open_tar(data: &[u8]) -> Box<dyn Read + '_> {
    if data.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::GzDecoder::new(data))
    } else {
        Box::new(data)
    }
}

pub type Files = BTreeMap<String, Vec<u8>>;

pub fn archive_files(data: &[u8], prefix: &str) -> R<Files> {
    let mut archive = tar::Archive::new(open_tar(data));
    let mut files = Files::new();
    let mut total: u64 = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind == tar::EntryType::XGlobalHeader {
            continue;
        }
        let name = String::from_utf8(entry.path_bytes().into_owned())?;
        require(
            !name.starts_with('/') && !name.split('/').any(|part| part == ".."),
            "Unsafe archive path",
        )?;
        let is_file = matches!(
            kind,
            tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse
        );
        require(
            is_file || kind == tar::EntryType::Directory,
            "Archive contains a link or special file",
        )?;
        if kind == tar::EntryType::Directory {
            continue;
        }
        require(name.starts_with(prefix), "Unexpected archive prefix")?;
        let relative = name[prefix.len()..].to_string();
        require(
            !relative.is_empty() && !files.contains_key(&relative),
            "Duplicate archive file",
        )?;
        total += entry.header().size()?;
        require(
            total < 200_000_000,
            "Archive exceeds the expected package size",
        )?;
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        files.insert(relative, content);
    }
    Ok(files)
}

pub fn unpack(files: &Files, destination: &Path) -> R<()> {
    for (name, data) in files {
        let target = destination.join(name);
        fs::create_dir_all(target.parent().unwrap_or(destination))?;
        fs::write(target, data)?;
    }
    Ok(())
}

fn is_dir_following(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn collect_source(
    root: &Path,
    dir: &Path,
    at_root: bool,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> R<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let symlink = fs::symlink_metadata(&path)?.file_type().is_symlink();
        let is_dir = is_dir_following(&path);
        if is_dir {
            if name == "__pycache__"
                || (at_root && matches!(name.as_str(), ".git" | ".ccid" | "target"))
            {
                continue;
            }
            require(!symlink, "Release source contains a symbolic link")?;
            collect_source(root, &path, false, files)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| Fail::Other("ValueError"))?;
            if relative.components().next().map(|c| c.as_os_str()) == Some(".git".as_ref()) {
                continue;
            }
            require(!symlink, "Release source contains a symbolic link")?;
            files.insert(
                relative.to_string_lossy().replace('\\', "/"),
                fs::read(&path)?,
            );
        }
    }
    Ok(())
}

pub fn working_source_files(root: &Path) -> R<Files> {
    let mut files = Files::new();
    collect_source(root, root, true, &mut files)?;
    Ok(files)
}

/// Git and Crow can encode identical sources into different tar bytes. Bind every path,
/// executable bit and payload, retaining the raw transport checksum separately rather than
/// forcing a rebuild after a provider switch.
pub fn source_content_identity(root: &Path, source_bytes: &[u8], files: &Files) -> R<String> {
    let mut archive = tar::Archive::new(open_tar(source_bytes));
    let mut executable: BTreeMap<String, u32> = BTreeMap::new();
    for entry in archive.entries()? {
        let entry = entry?;
        if matches!(
            entry.header().entry_type(),
            tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse
        ) {
            let name = String::from_utf8(entry.path_bytes().into_owned())?;
            executable.insert(name, entry.header().mode()? & 0o111);
        }
    }
    for (name, mode) in &executable {
        let actual = fs::metadata(root.join(name))?.mode() & 0o111;
        require(
            actual == *mode,
            &format!("Release source executable mode differs: {name}"),
        )?;
    }
    let rows: Vec<Value> = files
        .iter()
        .map(|(name, data)| match executable.get(name) {
            Some(mode) => Ok(json!([name, mode, digest(data)])),
            None => key_error(),
        })
        .collect::<R<_>>()?;
    Ok(digest(dump(&Value::Array(rows), Style::Compact).as_bytes()))
}

pub fn stable_identity(identity: &Map<String, Value>) -> Map<String, Value> {
    identity
        .iter()
        .filter(|(k, _)| k.as_str() != "driver_source_sha256")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn package_table(manifest: &toml::Value) -> R<&toml::value::Table> {
    match manifest.get("package").and_then(toml::Value::as_table) {
        Some(t) => Ok(t),
        None => key_error(),
    }
}

fn parse_toml(text: &str) -> R<toml::Value> {
    Ok(text.parse::<toml::Value>()?)
}

pub fn source_identity(root: &Path, env: &Env, default_tag: bool) -> R<Map<String, Value>> {
    let manifest = parse_toml(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let package = package_table(&manifest)?;
    let version = match package.get("version").and_then(toml::Value::as_str) {
        Some(v) => v.to_string(),
        None => return key_error(),
    };
    require(
        package.get("name").and_then(toml::Value::as_str) == Some("ctypst") && is_version(&version),
        "Unexpected release identity",
    )?;
    let commit = match env.nonempty("CI_COMMIT_SHA") {
        Some(c) => c.to_string(),
        None => run_capture(&["git", "rev-parse", "HEAD"], root)?,
    };
    require(is_hex40(&commit), "A full source commit is required")?;
    let tag = match env.nonempty("RELEASE_TAG") {
        Some(t) => t.to_string(),
        None if default_tag => format!("v{version}"),
        None => String::new(),
    };
    require(
        tag == format!("v{version}"),
        "RELEASE_TAG must match the committed package version",
    )?;
    let (source_bytes, source_hash) = if let Some(archive) = env.nonempty("SOURCE_ARCHIVE") {
        let bytes = fs::read(archive)?;
        let hash = digest(&bytes);
        require(
            Some(hash.as_str()) == env.get("SOURCE_SHA256"),
            "Source archive hash mismatch",
        )?;
        (bytes, hash)
    } else {
        let bytes = check_output(&["git", "archive", "--format=tar", &commit], root)?;
        let hash = digest(&bytes);
        (bytes, hash)
    };
    require(
        tar_commit_id(&source_bytes)? == commit,
        "Source archive commit mismatch",
    )?;
    let expected = archive_files(&source_bytes, "")?;
    let actual = working_source_files(root)?;
    require(
        actual.keys().eq(expected.keys()),
        "Release source file inventory differs from its committed archive",
    )?;
    for (name, data) in &actual {
        require(
            expected.get(name) == Some(data),
            &format!("Release source differs from its committed archive: {name}"),
        )?;
    }
    for path in ["package.json", "jsr.json"] {
        let found = read_json(&root.join("js/@corbet-labs/ctypst").join(path))?;
        match found.get("version") {
            Some(v) => require(
                v.as_str() == Some(version.as_str()),
                "JavaScript version differs from Cargo",
            )?,
            None => return key_error(),
        }
    }
    let content = source_content_identity(root, &source_bytes, &expected)?;
    let lock = digest(&fs::read(root.join("Cargo.lock"))?);
    let mut identity = Map::new();
    identity.insert("schema".into(), json!(1));
    identity.insert("repository".into(), json!(REPOSITORY));
    identity.insert("version".into(), json!(version));
    identity.insert("tag".into(), json!(tag));
    identity.insert("driver_commit".into(), json!(commit));
    identity.insert("driver_source_sha256".into(), json!(source_hash));
    identity.insert("driver_content_sha256".into(), json!(content));
    identity.insert("lock_sha256".into(), json!(lock));
    Ok(identity)
}

fn id_str<'a>(identity: &'a Map<String, Value>, name: &str) -> R<&'a str> {
    match identity.get(name).and_then(Value::as_str) {
        Some(v) => Ok(v),
        None => key_error(),
    }
}

pub fn artifacts_directory(identity: &Map<String, Value>, root: &Path, env: &Env) -> R<PathBuf> {
    let _ = root;
    let directory = if let Some(supplied) = env.nonempty("RELEASE_ARTIFACT_DIR") {
        let directory = PathBuf::from(supplied);
        require(
            directory.is_absolute(),
            "RELEASE_ARTIFACT_DIR must be absolute",
        )?;
        directory
    } else {
        let target = env.nonempty("CARGO_TARGET_DIR").map(PathBuf::from);
        require(
            target.as_ref().is_some_and(|t| t.is_absolute()),
            "An absolute persistent CARGO_TARGET_DIR or RELEASE_ARTIFACT_DIR is required",
        )?;
        target
            .unwrap()
            .join("releases")
            .join(id_str(identity, "driver_content_sha256")?)
    };
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

/// An exclusive, non-blocking lock held for as long as the returned file lives.
pub fn release_lock(directory: &Path) -> R<fs::File> {
    let stream = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(directory.join("release.lock"))?;
    // SAFETY: flock on a descriptor we own.
    let result = unsafe { libc::flock(stream.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(stream)
}

// ---------------------------------------------------------------- crate checks

fn tget<'a>(table: &'a toml::Value, name: &str) -> Option<&'a toml::Value> {
    table.get(name)
}

pub fn dependencies(manifest: &toml::Value) -> R<Vec<Value>> {
    let mut result: Vec<(String, String, String, Value)> = Vec::new();
    let mut append = |table: &toml::Value, target: Option<&str>| -> R<()> {
        for (section, kind) in [
            ("dependencies", "normal"),
            ("dev-dependencies", "dev"),
            ("build-dependencies", "build"),
        ] {
            let Some(entries) = tget(table, section).and_then(toml::Value::as_table) else {
                continue;
            };
            for (name, value) in entries {
                let spec: toml::value::Table = match value {
                    toml::Value::String(_) => {
                        let mut t = toml::value::Table::new();
                        t.insert("version".into(), value.clone());
                        t
                    }
                    toml::Value::Table(t) => t.clone(),
                    _ => return Err(Fail::Other("AttributeError")),
                };
                let version = spec.get("version").and_then(toml::Value::as_str);
                let unresolved = ["path", "git", "registry", "registry-index", "workspace"]
                    .iter()
                    .any(|k| spec.contains_key(*k));
                require(
                    version.is_some() && !unresolved,
                    "Unresolved registry dependency",
                )?;
                let renamed = spec.get("package").and_then(toml::Value::as_str);
                let row = json!({
                    "name": renamed.unwrap_or(name),
                    "version_req": version.unwrap_or_default(),
                    "features": spec.get("features").map(serde_json::to_value).transpose()?.unwrap_or(json!([])),
                    "optional": spec.get("optional").and_then(toml::Value::as_bool).unwrap_or(false),
                    "default_features": spec.get("default-features").and_then(toml::Value::as_bool).unwrap_or(true),
                    "target": target,
                    "kind": kind,
                    "registry": null,
                    "explicit_name_in_toml": if renamed.is_some() { json!(name) } else { Value::Null },
                });
                result.push((
                    target.unwrap_or("").to_string(),
                    kind.to_string(),
                    renamed.unwrap_or(name).to_string(),
                    row,
                ));
            }
        }
        Ok(())
    };
    append(manifest, None)?;
    if let Some(targets) = tget(manifest, "target").and_then(toml::Value::as_table) {
        for (target, table) in targets {
            append(table, Some(target))?;
        }
    }
    result.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    Ok(result.into_iter().map(|r| r.3).collect())
}

fn file_text(files: &Files, name: &str) -> R<String> {
    match files.get(name) {
        Some(d) => Ok(String::from_utf8(d.clone())?),
        None => key_error(),
    }
}

pub fn crate_metadata(files: &Files, version: &str) -> R<()> {
    let manifest = parse_toml(&file_text(files, "Cargo.toml")?)?;
    let original = parse_toml(&file_text(files, "Cargo.toml.orig")?)?;
    let package = package_table(&manifest)?;
    let original_package = package_table(&original)?;
    require(
        package.get("name").and_then(toml::Value::as_str) == Some("ctypst")
            && package.get("version").and_then(toml::Value::as_str) == Some(version),
        "Crate version mismatch",
    )?;
    for key in [
        "name",
        "version",
        "edition",
        "rust-version",
        "authors",
        "description",
        "repository",
        "homepage",
        "documentation",
        "license",
        "license-file",
        "keywords",
        "categories",
        "links",
    ] {
        require(
            package.get(key) == original_package.get(key),
            &format!("Normalized crate metadata differs: {key}"),
        )?;
    }
    require(
        dependencies(&manifest)? == dependencies(&original)?,
        "Normalized crate dependencies differ",
    )?;
    require(
        tget(&manifest, "features") == tget(&original, "features"),
        "Normalized features differ",
    )?;
    let empty = |v: Option<&toml::Value>| match v {
        None => true,
        Some(toml::Value::Table(t)) => t.is_empty(),
        Some(toml::Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    require(
        empty(tget(&manifest, "patch")) && empty(tget(&manifest, "replace")),
        "Local overrides in crate",
    )?;
    require(
        package.get("repository").and_then(toml::Value::as_str)
            == Some("https://git.corbet.ch/corbet-fsl/ctypst"),
        "Unexpected crate repository",
    )?;
    // The readme named by the manifest must be in the crate.
    match package.get("readme").and_then(toml::Value::as_str) {
        Some(readme) if files.contains_key(readme) => Ok(()),
        _ => key_error(),
    }
}

pub fn check_crate(data: &[u8], source: &Files, version: &str, producing_commit: &str) -> R<()> {
    let files = archive_files(data, &format!("ctypst-{version}/"))?;
    crate_metadata(&files, version)?;
    for (name, content) in &files {
        if name == "Cargo.toml" {
            continue;
        }
        if name == ".cargo_vcs_info.json" {
            let vcs: Value = serde_json::from_slice(content)?;
            let git = vcs.get("git");
            require(
                git.and_then(|g| g.get("sha1")).and_then(Value::as_str) == Some(producing_commit)
                    && git
                        .and_then(|g| g.get("dirty"))
                        .is_none_or(|d| d.is_null() || d == &Value::Bool(false) || d == &json!(0)),
                "Cargo VCS provenance differs from the clean producing commit",
            )?;
            continue;
        }
        let original_name = if name == "Cargo.toml.orig" {
            "Cargo.toml"
        } else {
            name
        };
        require(
            source.get(original_name) == Some(content),
            &format!("Crate member differs from producing source: {name}"),
        )?;
    }
    require(
        files.get("Cargo.lock") == source.get("Cargo.lock"),
        "Crate lock differs from producing source",
    )
}

pub fn load_component(
    directory: &Path,
    identity: &Map<String, Value>,
    component: &str,
) -> R<Value> {
    let manifest = read_json(&directory.join(format!("{component}.json")))?;
    require(
        stable_identity(identity)
            .iter()
            .all(|(k, v)| manifest.get(k) == Some(v)),
        "Artifact receipt belongs to another source or lock",
    )?;
    let Some(artifacts) = manifest.get("artifacts").and_then(Value::as_object) else {
        return key_error();
    };
    for (name, expected) in artifacts {
        require(
            !name.contains('/') && name != "." && name != "..",
            "Unsafe artifact name",
        )?;
        let path = directory.join(name);
        let meta = fs::symlink_metadata(&path);
        let ok = meta.as_ref().is_ok_and(|m| m.file_type().is_file())
            && Some(digest(&fs::read(&path)?).as_str()) == expected.as_str();
        require(ok, &format!("Artifact changed: {name}"))?;
    }
    Ok(manifest)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::Bool(true)) => true,
    }
}

fn walk_files(base: &Path, dir: &Path, out: &mut Files) -> R<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        let meta = fs::metadata(&path)?;
        if meta.is_dir() {
            walk_files(base, &path, out)?;
        } else if meta.is_file() {
            let relative = path
                .strip_prefix(base)
                .map_err(|_| Fail::Other("ValueError"))?;
            out.insert(relative.to_string_lossy().into_owned(), fs::read(&path)?);
        }
    }
    Ok(())
}

fn rustup_target(root: &Path, env: &Env) -> PathBuf {
    env.nonempty("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

pub fn prepare_cargo(
    directory: &Path,
    identity: &Map<String, Value>,
    root: &Path,
    env: &Env,
    out: &mut Vec<String>,
) -> R<()> {
    if directory.join("cargo.json").exists() {
        load_component(directory, identity, "cargo")?;
        out.push("Reusing verified Cargo artifact".into());
        return Ok(());
    }
    let version = id_str(identity, "version")?.to_string();
    let crate_name = format!("ctypst-{version}.crate");
    let target = directory.join(&crate_name);
    let mut provenance = Map::new();
    if let Some(imported) = env.nonempty("RELEASE_REUSE_CRATE") {
        // Import carries its own audited producing-source receipt, not a claim that a new helper
        // commit or a historic tag produced the old bytes.
        let proof_path = PathBuf::from(env.required("RELEASE_REUSE_RECEIPT")?);
        let proof_bytes = fs::read(&proof_path)?;
        require(
            Some(digest(&proof_bytes).as_str()) == env.get("RELEASE_REUSE_RECEIPT_SHA256"),
            "Imported proof checksum mismatch",
        )?;
        let proof = read_json(&proof_path)?;
        let text = |name: &str| proof.get(name).and_then(Value::as_str).unwrap_or("");
        for name in ["producing_commit", "tag_commit"] {
            require(
                is_hex40(text(name)),
                "Imported proof needs producing and tag commits",
            )?;
        }
        require(
            proof.get("version").and_then(Value::as_str) == Some(version.as_str())
                && proof.get("checks_passed") == Some(&Value::Bool(true))
                && truthy(proof.get("evidence")),
            "Imported artifact lacks reviewed validation evidence",
        )?;
        let imported_bytes = fs::read(imported)?;
        require(
            Some(digest(&imported_bytes).as_str())
                == proof.get("crate_sha256").and_then(Value::as_str),
            "Imported crate checksum mismatch",
        )?;
        let source_bytes = fs::read(env.required("RELEASE_REUSE_SOURCE_ARCHIVE")?)?;
        require(
            Some(digest(&source_bytes).as_str())
                == proof.get("source_sha256").and_then(Value::as_str),
            "Imported source checksum mismatch",
        )?;
        let producing_commit = tar_commit_id(&source_bytes)?;
        require(
            producing_commit == text("producing_commit"),
            "Imported source archive commit mismatch",
        )?;
        let source = archive_files(&source_bytes, "")?;
        require(
            source.get("Cargo.toml") == Some(&fs::read(root.join("Cargo.toml"))?)
                && source.get("Cargo.lock") == Some(&fs::read(root.join("Cargo.lock"))?),
            "Imported package graph differs from release driver source",
        )?;
        let prefixes = ["src/", "fonts/", "typst/", "protocol/"];
        let imported_runtime: Files = source
            .iter()
            .filter(|(n, _)| prefixes.iter().any(|p| n.starts_with(p)))
            .map(|(n, d)| (n.clone(), d.clone()))
            .collect();
        let mut driver_runtime = Files::new();
        for prefix in prefixes {
            walk_files(root, &root.join(prefix), &mut driver_runtime)?;
        }
        require(
            imported_runtime == driver_runtime,
            "Imported runtime inventory or input differs from release driver source",
        )?;
        check_crate(&imported_bytes, &source, &version, &producing_commit)?;
        fs::copy(imported, &target)?;
        provenance.insert("producing_commit".into(), proof["producing_commit"].clone());
        provenance.insert(
            "producing_source_sha256".into(),
            proof["source_sha256"].clone(),
        );
        provenance.insert("tag_commit".into(), proof["tag_commit"].clone());
        provenance.insert("verification".into(), proof["evidence"].clone());
        provenance.insert("import_receipt_sha256".into(), json!(digest(&proof_bytes)));
    } else {
        run(&["bash", "scripts/ci.sh", "quality"], root, None, false)?;
        run(&["bash", "scripts/ci.sh", "test"], root, None, false)?;
        let cargo_target = rustup_target(root, env);
        // cargo publish --dry-run retains the exact upload archive here.
        let candidates = [
            cargo_target.join("package/tmp-crate").join(&crate_name),
            cargo_target.join("package").join(&crate_name),
            cargo_target.join("package/tmp-registry").join(&crate_name),
        ];
        let candidate = candidates.iter().find(|p| p.is_file());
        require(
            candidate.is_some(),
            "Cargo dry-run did not retain an upload crate; preserve the job and inspect it",
        )?;
        let candidate = candidate.unwrap();
        let commit = id_str(identity, "driver_commit")?.to_string();
        let source = if let Some(archive) = env.nonempty("SOURCE_ARCHIVE") {
            archive_files(&fs::read(archive)?, "")?
        } else {
            let listing = run_capture(&["git", "ls-files"], root)?;
            let mut files = Files::new();
            for name in listing.lines() {
                files.insert(
                    name.to_string(),
                    check_output(&["git", "show", &format!("{commit}:{name}")], root)?,
                );
            }
            files
        };
        check_crate(&fs::read(candidate)?, &source, &version, &commit)?;
        fs::copy(candidate, &target)?;
        provenance.insert("producing_commit".into(), json!(commit));
        provenance.insert(
            "producing_source_sha256".into(),
            json!(id_str(identity, "driver_source_sha256")?),
        );
        provenance.insert("tag_commit".into(), json!(commit));
        provenance.insert(
            "verification".into(),
            json!(["quality", "test", "cargo-publish-dry-run"]),
        );
        provenance.insert(
            "rustc".into(),
            json!(run_capture(&["rustc", "--version", "--verbose"], root)?),
        );
        provenance.insert(
            "cargo".into(),
            json!(run_capture(&["cargo", "--version"], root)?),
        );
    }
    let mut receipt = identity.clone();
    receipt.extend(provenance);
    receipt.insert(
        "artifacts".into(),
        json!({ crate_name.clone(): digest(&fs::read(&target)?) }),
    );
    write_json(&directory.join("cargo.json"), &Value::Object(receipt))
}

fn which(tool: &str, env: &Env) -> bool {
    env.get("PATH").is_some_and(|path| {
        std::env::split_paths(path).any(|dir| {
            fs::metadata(dir.join(tool))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

pub fn copy_tree(src: &Path, dst: &Path, ignore: &dyn Fn(&str) -> bool) -> R<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if ignore(&name) {
            continue;
        }
        let path = entry.path();
        let meta = fs::metadata(&path)?;
        if meta.is_dir() {
            copy_tree(&path, &dst.join(&name), ignore)?;
        } else {
            fs::copy(&path, dst.join(&name))?;
        }
    }
    Ok(())
}

fn list_files(dir: &Path, out: &mut Vec<PathBuf>) -> R<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if fs::metadata(&path)?.is_dir() {
            list_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn last_word(text: &str) -> &str {
    text.split_whitespace().last().unwrap_or("")
}

pub fn prepare_javascript(
    directory: &Path,
    identity: &Map<String, Value>,
    root: &Path,
    env: &Env,
    out: &mut Vec<String>,
) -> R<()> {
    if directory.join("javascript.json").exists() {
        load_component(directory, identity, "javascript")?;
        out.push("Reusing verified JavaScript artifacts".into());
        return Ok(());
    }
    for tool in ["bun", "wasm-bindgen", "wasm-opt", "deno"] {
        require(
            which(tool, env),
            &format!("Provisioned {tool} is required; this release command does not install tools"),
        )?;
    }
    let lock = parse_toml(&fs::read_to_string(root.join("Cargo.lock"))?)?;
    let mut bindgen = BTreeSet::new();
    if let Some(packages) = lock.get("package").and_then(toml::Value::as_array) {
        for item in packages {
            if item.get("name").and_then(toml::Value::as_str) == Some("wasm-bindgen") {
                match item.get("version").and_then(toml::Value::as_str) {
                    Some(v) => {
                        bindgen.insert(v.to_string());
                    }
                    None => return key_error(),
                }
            }
        }
    } else {
        return key_error();
    }
    require(
        bindgen.len() == 1
            && bindgen.contains(last_word(&run_capture(
                &["wasm-bindgen", "--version"],
                root,
            )?)),
        "Provisioned wasm-bindgen must match Cargo.lock",
    )?;
    let version = id_str(identity, "version")?.to_string();
    let scratch = match env.nonempty("TMPDIR") {
        Some(dir) => tempfile::Builder::new()
            .prefix("ctypst-release-js-")
            .tempdir_in(dir)?,
        None => tempfile::Builder::new()
            .prefix("ctypst-release-js-")
            .tempdir()?,
    };
    let stage = scratch.path();
    // Keep Bun's generated files out of the Cargo package source tree.
    for name in ["js", "typst", "fonts", "protocol"] {
        copy_tree(&root.join(name), &stage.join(name), &|n| {
            matches!(n, "node_modules" | "dist" | "wasm") || n.ends_with(".tgz")
        })?;
    }
    fs::copy(root.join("Cargo.toml"), stage.join("Cargo.toml"))?;
    let package = stage.join("js/@corbet-labs/ctypst");
    run(
        &["bun", "install", "--frozen-lockfile"],
        &package,
        None,
        false,
    )?;
    run(&["bash", "scripts/sync-assets.sh"], &package, None, false)?;
    run(
        &[
            "bun",
            "./node_modules/typescript/bin/tsc",
            "--noEmit",
            "-p",
            "tsconfig.json",
        ],
        &package,
        None,
        false,
    )?;
    run(&["bun", "scripts/conformance.mts"], &package, None, false)?;
    let mut build_env = env.clone();
    build_env.0.insert(
        "RUSTFLAGS".into(),
        "-C opt-level=z -C codegen-units=1".into(),
    );
    run(
        &[
            "cargo",
            "rustc",
            "--locked",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
            "--features",
            "wasm",
            "--crate-type",
            "cdylib",
        ],
        root,
        Some(&build_env),
        false,
    )?;
    let wasm = rustup_target(root, env).join("wasm32-unknown-unknown/release/ctypst.wasm");
    let optimized = stage.join("ctypst.wasm");
    run(
        &[
            "wasm-opt",
            "--enable-bulk-memory",
            "--enable-nontrapping-float-to-int",
            "-Oz",
            &wasm.to_string_lossy(),
            "-o",
            &optimized.to_string_lossy(),
        ],
        root,
        None,
        false,
    )?;
    require(
        fs::metadata(&optimized)?.len() < 26_214_400,
        "WASM exceeds the 25 MiB distribution budget",
    )?;
    for kind in ["web", "nodejs"] {
        run(
            &[
                "wasm-bindgen",
                "--target",
                kind,
                "--out-dir",
                &package.join("wasm").join(kind).to_string_lossy(),
                &optimized.to_string_lossy(),
            ],
            root,
            None,
            false,
        )?;
    }
    // wasm-bindgen emits CommonJS for nodejs; the enclosing package is ESM.
    fs::write(
        package.join("wasm/nodejs/package.json"),
        "{\"type\":\"commonjs\"}\n",
    )?;
    let packed = stage.join("packed");
    fs::create_dir(&packed)?;
    run(
        &[
            "bun",
            "pm",
            "pack",
            "--destination",
            &packed.to_string_lossy(),
        ],
        &package,
        None,
        false,
    )?;
    let mut archives: Vec<PathBuf> = fs::read_dir(&packed)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "tgz"))
        .collect();
    archives.sort();
    require(archives.len() == 1, "Expected exactly one npm archive")?;
    let npm = archive_files(&fs::read(&archives[0])?, "package/")?;
    let npm_package: Value = match npm.get("package.json") {
        Some(d) => serde_json::from_slice(d)?,
        None => return key_error(),
    };
    require(
        npm_package.get("version").and_then(Value::as_str) == Some(version.as_str()),
        "npm package version mismatch",
    )?;
    require(
        npm.keys().filter(|n| n.ends_with(".ttf")).count() == 16,
        "npm font inventory mismatch",
    )?;
    require(
        [
            "wasm/web/ctypst_bg.wasm",
            "wasm/nodejs/ctypst_bg.wasm",
            "wasm/nodejs/package.json",
        ]
        .iter()
        .all(|n| npm.contains_key(*n)),
        "Packaged WASM outputs are missing",
    )?;
    let smoke = stage.join("smoke");
    unpack(&npm, &smoke)?;
    run(
        &[
            "bun",
            &root
                .join("scripts/release-wasm-smoke.mjs")
                .to_string_lossy(),
            &smoke.to_string_lossy(),
            &root
                .join("protocol/measure-v1/requests.json")
                .to_string_lossy(),
        ],
        root,
        None,
        false,
    )?;
    let archive_name = archives[0]
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_default();
    let npm_target = directory.join(&archive_name);
    fs::copy(&archives[0], &npm_target)?;
    let jsr = stage.join("jsr");
    fs::create_dir(&jsr)?;
    for name in ["jsr.json", "README.md"] {
        fs::copy(package.join(name), jsr.join(name))?;
    }
    copy_tree(&package.join("src"), &jsr.join("src"), &|_| false)?;
    run(
        &[
            "deno",
            "publish",
            "--dry-run",
            "--allow-dirty",
            "--no-provenance",
            "--node-modules-dir=none",
        ],
        &jsr,
        None,
        false,
    )?;
    let jsr_target = directory.join(format!("ctypst-{version}-jsr.tar.gz"));
    {
        let encoder = flate2::write::GzEncoder::new(
            fs::File::create(&jsr_target)?,
            flate2::Compression::best(),
        );
        let mut builder = tar::Builder::new(encoder);
        let mut files = Vec::new();
        list_files(&jsr, &mut files)?;
        files.sort();
        for path in files {
            let relative = path
                .strip_prefix(&jsr)
                .map_err(|_| Fail::Other("ValueError"))?;
            builder.append_path_with_name(&path, relative)?;
        }
        builder.into_inner()?.finish()?;
    }
    let mut tools = Map::new();
    for tool in ["rustc", "cargo", "bun", "wasm-bindgen", "wasm-opt", "deno"] {
        tools.insert(tool.into(), json!(run_capture(&[tool, "--version"], root)?));
    }
    let commit = id_str(identity, "driver_commit")?.to_string();
    let mut receipt = identity.clone();
    receipt.insert("producing_commit".into(), json!(commit));
    receipt.insert(
        "producing_source_sha256".into(),
        json!(id_str(identity, "driver_source_sha256")?),
    );
    receipt.insert("tag_commit".into(), json!(commit));
    receipt.insert(
        "verification".into(),
        json!([
            "typescript",
            "measurement-conformance",
            "packaged-wasm-smoke",
            "jsr-dry-run"
        ]),
    );
    receipt.insert("tools".into(), Value::Object(tools));
    let mut artifacts = Map::new();
    for path in [&npm_target, &jsr_target] {
        artifacts.insert(
            path.file_name().unwrap().to_string_lossy().into_owned(),
            json!(digest(&fs::read(path)?)),
        );
    }
    receipt.insert("artifacts".into(), Value::Object(artifacts));
    write_json(&directory.join("javascript.json"), &Value::Object(receipt))
}

/// The package-version consistency gate: every package manifest carries the crate version.
pub fn check_versions(root: &Path) -> R<String> {
    let manifest = parse_toml(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let version = match package_table(&manifest)?
        .get("version")
        .and_then(toml::Value::as_str)
    {
        Some(v) => v.to_string(),
        None => return key_error(),
    };
    for path in [
        "js/@corbet-labs/ctypst/package.json",
        "js/@corbet-labs/ctypst/jsr.json",
    ] {
        let found = read_json(&root.join(path))?;
        match found.get("version") {
            Some(v) if v.as_str() == Some(version.as_str()) => {}
            Some(_) => {
                return Err(Fail::Msg(format!(
                    "{path} does not match crate version {version}"
                )));
            }
            None => return key_error(),
        }
    }
    for (path, table) in [
        ("bindings/python/Cargo.toml", "package"),
        ("bindings/python/pyproject.toml", "project"),
    ] {
        let parsed = parse_toml(&fs::read_to_string(root.join(path))?)?;
        match parsed
            .get(table)
            .and_then(|t| t.get("version"))
            .and_then(toml::Value::as_str)
        {
            Some(v) if v == version => {}
            Some(_) => {
                return Err(Fail::Msg(format!(
                    "{path} does not match crate version {version}"
                )));
            }
            None => return key_error(),
        }
    }
    Ok(format!("All package versions match {version}"))
}

const USAGE: &str = "usage: ctypst-release {source|prepare} [cargo|javascript|all]\n       ctypst-release versions [ROOT]";

/// Run the command line. Returns the line to print on success.
pub fn main_with(args: &[String], root: &Path, env: &Env) -> R<Option<String>> {
    let usage = || Fail::Usage(USAGE.to_string());
    let stage = args.first().map(String::as_str).ok_or_else(usage)?;
    if stage == "versions" {
        if args.len() > 2 {
            return Err(usage());
        }
        let target = args
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| root.to_path_buf());
        return check_versions(&target).map(Some);
    }
    if !matches!(stage, "source" | "prepare") || args.len() > 2 {
        return Err(usage());
    }
    let component = args.get(1).map(String::as_str).unwrap_or("all");
    if !matches!(component, "cargo" | "javascript" | "all") {
        return Err(usage());
    }
    let identity = source_identity(root, env, stage == "source")?;
    if stage == "source" {
        return Ok(Some(dump(&Value::Object(identity), Style::Default)));
    }
    let directory = artifacts_directory(&identity, root, env)?;
    let components: &[&str] = if component == "all" {
        &["cargo", "javascript"]
    } else {
        std::slice::from_ref(&component)
    };
    let mut lines = Vec::new();
    let _lock = release_lock(&directory)?;
    require(
        !TOKENS.iter().any(|name| env.nonempty(name).is_some()),
        "Publication credentials must be absent from preparation",
    )?;
    for component in components {
        if *component == "cargo" {
            prepare_cargo(&directory, &identity, root, env, &mut lines)?;
        } else {
            prepare_javascript(&directory, &identity, root, env, &mut lines)?;
        }
    }
    let mut summary = Map::new();
    summary.insert("stage".into(), json!(stage));
    summary.insert("component".into(), json!(component));
    summary.insert("artifacts".into(), json!(directory.to_string_lossy()));
    summary.extend(identity);
    lines.push(dump(&Value::Object(summary), Style::Default));
    Ok(Some(lines.join("\n")))
}
