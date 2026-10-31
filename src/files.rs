use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
use typst::syntax::package::{PackageManifest, PackageSpec};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst_as_lib::file_resolver::FileResolver;

use crate::{Error, Limits, Result};

fn file_id(path: &str) -> Result<FileId> {
    let virtual_path =
        VirtualPath::new(path).map_err(|_| Error::InvalidVirtualPath(path.to_owned()))?;
    Ok(RootedPath::new(VirtualRoot::Project, virtual_path).intern())
}

fn denied(message: impl Into<String>) -> FileError {
    FileError::Other(Some(message.into().into()))
}

fn not_found(id: FileId) -> FileError {
    FileError::NotFound(id.vpath().get_without_slash().into())
}

pub(crate) fn invalid_package(
    subject: impl std::fmt::Display,
    reason: impl std::fmt::Display,
) -> Error {
    Error::InvalidVirtualPath(format!("{subject}: {reason}"))
}

/// Package-relative paths and bytes of one application-supplied Typst package.
pub(crate) type PackageFileList = Vec<(String, Vec<u8>)>;

/// Validate one package before it is registered and return its parsed spec.
///
/// Paths must already be normalized and package-relative, so no two names can
/// reach the same file and none can leave the package root. The manifest must
/// describe exactly this spec and name a supplied entrypoint, as Typst would
/// otherwise only discover a mismatch while importing.
pub(crate) fn check_package(
    spec: &str,
    files: &[(String, Vec<u8>)],
    limits: Limits,
) -> Result<PackageSpec> {
    let parsed = spec
        .parse::<PackageSpec>()
        .map_err(|reason| invalid_package(spec, reason))?;
    check_limit(
        parsed.to_string(),
        "package file count",
        files.len(),
        limits.max_files,
    )?;
    let mut paths = BTreeSet::new();
    let mut bytes = 0_usize;
    for (path, data) in files {
        let resource = format!("{parsed}/{path}");
        check_limit(&resource, "path bytes", path.len(), limits.max_path_bytes)?;
        check_limit(&resource, "file bytes", data.len(), limits.max_file_bytes)?;
        let contained = VirtualPath::new(path).is_ok_and(|normalized| {
            !normalized.is_root() && normalized.get_without_slash() == path
        });
        if !contained {
            return Err(invalid_package(
                &resource,
                "package paths must be normalized, relative, and contained",
            ));
        }
        if !paths.insert(path.as_str()) {
            return Err(invalid_package(&resource, "package file is supplied twice"));
        }
        bytes = bytes.saturating_add(data.len());
        check_limit(
            parsed.to_string(),
            "package data bytes",
            bytes,
            limits.max_total_bytes,
        )?;
    }
    let manifest_path = format!("{parsed}/typst.toml");
    let manifest = files
        .iter()
        .find_map(|(path, data)| (path == "typst.toml").then_some(data))
        .ok_or_else(|| invalid_package(&manifest_path, "package manifest is missing"))?;
    let manifest = std::str::from_utf8(manifest)
        .map_err(|_| invalid_package(&manifest_path, "package manifest is not UTF-8"))?;
    let manifest = toml::from_str::<PackageManifest>(manifest).map_err(|error| {
        invalid_package(
            &manifest_path,
            format!("package manifest is malformed ({})", error.message()),
        )
    })?;
    manifest
        .validate(&parsed)
        .map_err(|reason| invalid_package(&manifest_path, reason))?;
    // Typst resolves the entrypoint relative to the manifest at the root.
    let entrypoint = VirtualPath::new(manifest.package.entrypoint.as_str())
        .map_err(|reason| invalid_package(&manifest_path, reason))?;
    if !paths.contains(entrypoint.get_without_slash()) {
        return Err(invalid_package(
            &manifest_path,
            format!(
                "package entrypoint `{}` is not a supplied file",
                manifest.package.entrypoint
            ),
        ));
    }
    Ok(parsed)
}

#[derive(Default)]
struct PackageSet {
    specs: HashSet<PackageSpec>,
    files: HashMap<FileId, Bytes>,
}

/// Immutable in-memory Typst packages registered by the application.
///
/// Lookups are keyed by interned file IDs; nothing iterates the maps, so their
/// order cannot reach diagnostics or documents.
#[derive(Clone, Default)]
pub(crate) struct Packages {
    set: Arc<PackageSet>,
}

impl Packages {
    pub(crate) fn new(
        packages: impl IntoIterator<Item = (String, PackageFileList)>,
        limits: Limits,
    ) -> Result<Self> {
        let mut set = PackageSet::default();
        let mut bytes = 0_usize;
        for (spec, files) in packages {
            let parsed = check_package(&spec, &files, limits)?;
            if set.specs.contains(&parsed) {
                return Err(invalid_package(&parsed, "package is already registered"));
            }
            check_limit(
                "registered packages",
                "package file count",
                set.files.len().saturating_add(files.len()),
                limits.max_files,
            )?;
            bytes = files
                .iter()
                .map(|(_, data)| data.len())
                .fold(bytes, usize::saturating_add);
            check_limit(
                "registered packages",
                "package data bytes",
                bytes,
                limits.max_total_bytes,
            )?;
            for (path, data) in files {
                let virtual_path =
                    VirtualPath::new(&path).map_err(|reason| invalid_package(&parsed, reason))?;
                let id =
                    RootedPath::new(VirtualRoot::Package(parsed.clone()), virtual_path).intern();
                set.files.insert(id, Bytes::new(data));
            }
            set.specs.insert(parsed);
        }
        Ok(Self { set: Arc::new(set) })
    }

    fn resolve_binary(&self, spec: &PackageSpec, id: FileId) -> FileResult<Bytes> {
        if !self.set.specs.contains(spec) {
            return Err(denied(format!(
                "Typst package {spec} is not registered with this engine; \
                 package downloads are disabled"
            )));
        }
        self.set
            .files
            .get(&id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    fn resolve_source(&self, spec: &PackageSpec, id: FileId) -> FileResult<Source> {
        let bytes = self.resolve_binary(spec, id)?;
        let source = std::str::from_utf8(&bytes).map_err(|_| FileError::InvalidUtf8)?;
        Ok(Source::new(
            id,
            source.trim_start_matches('\u{feff}').to_owned(),
        ))
    }
}

#[derive(Clone, Default)]
pub(crate) struct VirtualFileSet {
    sources: HashMap<FileId, Source>,
    binaries: HashMap<FileId, Bytes>,
    bytes: usize,
}

#[derive(Clone)]
pub(crate) struct VirtualFiles {
    files: Arc<RwLock<VirtualFileSet>>,
    packages: Packages,
    limits: Limits,
}

impl VirtualFiles {
    pub(crate) fn new(limits: Limits, packages: Packages) -> Self {
        Self {
            files: Arc::default(),
            packages,
            limits,
        }
    }

    pub(crate) fn apply_updates(
        &self,
        sources: impl IntoIterator<Item = (String, String)>,
        binaries: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Option<VirtualFileSet>> {
        let sources = sources
            .into_iter()
            .map(|(path, source)| {
                self.check_file(&path, source.len())?;
                Ok((file_id(&path)?, source))
            })
            .collect::<Result<Vec<_>>>()?;
        let binaries = binaries
            .into_iter()
            .map(|(path, binary)| {
                self.check_file(&path, binary.len())?;
                Ok((file_id(&path)?, binary))
            })
            .collect::<Result<Vec<_>>>()?;
        if sources.is_empty() && binaries.is_empty() {
            return Ok(None);
        }
        let mut files = self.files.write().map_err(|_| Error::Poisoned)?;
        let mut next = files.clone();
        for (id, source) in sources {
            next.sources.insert(id, Source::new(id, source));
        }
        for (id, binary) in binaries {
            next.binaries.insert(id, Bytes::new(binary));
        }
        next.bytes = next
            .sources
            .values()
            .map(|source| source.text().len())
            .chain(next.binaries.values().map(Bytes::len))
            .fold(0_usize, usize::saturating_add);
        let next_files = next.sources.len().saturating_add(next.binaries.len());
        check_limit(
            "virtual files",
            "virtual file count",
            next_files,
            self.limits.max_files,
        )?;
        check_limit(
            "virtual files",
            "virtual data bytes",
            next.bytes,
            self.limits.max_total_bytes,
        )?;
        Ok(Some(std::mem::replace(&mut *files, next)))
    }

    pub(crate) fn restore(&self, snapshot: VirtualFileSet) -> Result<()> {
        *self.files.write().map_err(|_| Error::Poisoned)? = snapshot;
        Ok(())
    }

    fn check_file(&self, path: &str, bytes: usize) -> Result<()> {
        check_limit(path, "path bytes", path.len(), self.limits.max_path_bytes)?;
        check_limit(path, "file bytes", bytes, self.limits.max_file_bytes)
    }
}

impl FileResolver for VirtualFiles {
    fn resolve_binary(&self, id: FileId) -> FileResult<Cow<'_, Bytes>> {
        if let VirtualRoot::Package(spec) = id.root() {
            return self.packages.resolve_binary(spec, id).map(Cow::Owned);
        }
        self.files
            .read()
            .map_err(|_| denied("virtual file lock is poisoned"))?
            .binaries
            .get(&id)
            .cloned()
            .map(Cow::Owned)
            .ok_or_else(|| not_found(id))
    }

    fn resolve_source(&self, id: FileId) -> FileResult<Cow<'_, Source>> {
        if let VirtualRoot::Package(spec) = id.root() {
            return self.packages.resolve_source(spec, id).map(Cow::Owned);
        }
        self.files
            .read()
            .map_err(|_| denied("virtual file lock is poisoned"))?
            .sources
            .get(&id)
            .cloned()
            .map(Cow::Owned)
            .ok_or_else(|| not_found(id))
    }
}

#[derive(Default)]
struct ReadBudget {
    paths: HashMap<PathBuf, usize>,
    dependencies: BTreeSet<PathBuf>,
    bytes: usize,
}

impl ReadBudget {
    fn reset(&mut self) {
        self.paths.clear();
        self.dependencies.clear();
        self.bytes = 0;
    }
}

#[derive(Clone)]
pub(crate) struct SafeFsResolver {
    root: PathBuf,
    packages: Packages,
    limits: Limits,
    budget: Arc<Mutex<ReadBudget>>,
}

impl SafeFsResolver {
    pub(crate) fn new(root: &Path, limits: Limits, packages: Packages) -> Result<Self> {
        let root = root.canonicalize().map_err(|source| Error::InvalidRoot {
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            root,
            packages,
            limits,
            budget: Arc::default(),
        })
    }

    pub(crate) fn reset(&self) -> Result<()> {
        self.budget.lock().map_err(|_| Error::Poisoned)?.reset();
        Ok(())
    }

    pub(crate) fn dependencies(&self) -> Result<Vec<PathBuf>> {
        Ok(self
            .budget
            .lock()
            .map_err(|_| Error::Poisoned)?
            .dependencies
            .iter()
            .cloned()
            .collect())
    }

    fn observe(&self, path: &Path) -> FileResult<()> {
        let mut budget = self
            .budget
            .lock()
            .map_err(|_| denied("filesystem budget lock is poisoned"))?;
        // A file can contribute its lexical and canonical names. Keep failed
        // reads bounded too, while retaining the first over-budget dependency.
        if budget.dependencies.len() > self.limits.max_files.saturating_mul(2) {
            return Err(denied("Typst compilation consulted too many paths"));
        }
        budget.dependencies.insert(path.to_path_buf());
        Ok(())
    }

    fn resolve_bytes(&self, id: FileId) -> FileResult<Vec<u8>> {
        if !matches!(id.root(), VirtualRoot::Project) {
            return Err(denied("Typst package paths never reach the filesystem"));
        }
        if id.vpath().get_without_slash().len() > self.limits.max_path_bytes {
            return Err(denied("Typst path exceeds its byte limit"));
        }
        let lexical = id
            .vpath()
            .realize(&self.root)
            .map_err(|_| denied("Typst path escapes the configured root"))?;
        self.observe(&lexical)?;
        let path = lexical
            .canonicalize()
            .map_err(|error| FileError::from_io(error, &lexical))?;
        if !path.starts_with(&self.root) {
            return Err(denied(
                "Typst path escapes the configured root through a link",
            ));
        }
        self.observe(&path)?;
        let file = File::open(&path).map_err(|error| FileError::from_io(error, &path))?;
        let metadata = file
            .metadata()
            .map_err(|error| FileError::from_io(error, &path))?;
        if !metadata.is_file() {
            return Err(denied("Typst may only read regular files"));
        }
        let announced = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if announced > self.limits.max_file_bytes {
            return Err(denied(format!(
                "Typst file exceeds byte limit: {announced} > {}",
                self.limits.max_file_bytes
            )));
        }
        let read_limit = u64::try_from(self.limits.max_file_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::with_capacity(announced.min(self.limits.max_file_bytes));
        file.take(read_limit)
            .read_to_end(&mut bytes)
            .map_err(|error| FileError::from_io(error, &path))?;
        if bytes.len() > self.limits.max_file_bytes {
            return Err(denied(
                "Typst file grew beyond its byte limit while reading",
            ));
        }
        let mut budget = self
            .budget
            .lock()
            .map_err(|_| denied("filesystem budget lock is poisoned"))?;
        let existed = budget.paths.contains_key(&path);
        let previous = budget.paths.get(&path).copied().unwrap_or_default();
        let next_bytes = budget
            .bytes
            .saturating_sub(previous)
            .saturating_add(bytes.len());
        if budget.paths.len() + usize::from(!existed) > self.limits.max_files {
            return Err(denied("Typst compilation read too many files"));
        }
        if next_bytes > self.limits.max_total_bytes {
            return Err(denied("Typst compilation read too many bytes"));
        }
        budget.bytes = next_bytes;
        budget.paths.insert(path, bytes.len());
        Ok(bytes)
    }
}

// Typst reports the last resolver's error, so package roots answer identically
// here and never fall through to the filesystem.
impl FileResolver for SafeFsResolver {
    fn resolve_binary(&self, id: FileId) -> FileResult<Cow<'_, Bytes>> {
        if let VirtualRoot::Package(spec) = id.root() {
            return self.packages.resolve_binary(spec, id).map(Cow::Owned);
        }
        self.resolve_bytes(id).map(Bytes::new).map(Cow::Owned)
    }

    fn resolve_source(&self, id: FileId) -> FileResult<Cow<'_, Source>> {
        if let VirtualRoot::Package(spec) = id.root() {
            return self.packages.resolve_source(spec, id).map(Cow::Owned);
        }
        let bytes = self.resolve_bytes(id)?;
        let source = std::str::from_utf8(&bytes).map_err(|_| FileError::InvalidUtf8)?;
        Ok(Cow::Owned(Source::new(
            id,
            source.trim_start_matches('\u{feff}').to_owned(),
        )))
    }
}

fn check_limit(
    resource: impl Into<String>,
    limit_name: &'static str,
    actual: usize,
    maximum: usize,
) -> Result<()> {
    if actual > maximum {
        return Err(Error::Limit {
            resource: resource.into(),
            limit_name,
            actual,
            maximum,
        });
    }
    Ok(())
}
