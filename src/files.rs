use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
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

#[derive(Clone, Default)]
pub(crate) struct VirtualFileSet {
    sources: HashMap<FileId, Source>,
    binaries: HashMap<FileId, Bytes>,
    bytes: usize,
}

#[derive(Clone)]
pub(crate) struct VirtualFiles {
    files: Arc<RwLock<VirtualFileSet>>,
    limits: Limits,
}

impl VirtualFiles {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            files: Arc::default(),
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
        if !matches!(id.root(), VirtualRoot::Project) {
            return Err(denied("Typst package imports are disabled"));
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
        if !matches!(id.root(), VirtualRoot::Project) {
            return Err(denied("Typst package imports are disabled"));
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
    bytes: usize,
}

impl ReadBudget {
    fn reset(&mut self) {
        self.paths.clear();
        self.bytes = 0;
    }
}

#[derive(Clone)]
pub(crate) struct SafeFsResolver {
    root: PathBuf,
    limits: Limits,
    budget: Arc<Mutex<ReadBudget>>,
}

impl SafeFsResolver {
    pub(crate) fn new(root: &Path, limits: Limits) -> Result<Self> {
        let root = root.canonicalize().map_err(|source| Error::InvalidRoot {
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            root,
            limits,
            budget: Arc::default(),
        })
    }

    pub(crate) fn reset(&self) -> Result<()> {
        self.budget.lock().map_err(|_| Error::Poisoned)?.reset();
        Ok(())
    }

    fn resolve_bytes(&self, id: FileId) -> FileResult<Vec<u8>> {
        if !matches!(id.root(), VirtualRoot::Project) {
            return Err(denied("Typst package imports are disabled"));
        }
        if id.vpath().get_without_slash().len() > self.limits.max_path_bytes {
            return Err(denied("Typst path exceeds its byte limit"));
        }
        let lexical = id
            .vpath()
            .realize(&self.root)
            .map_err(|_| denied("Typst path escapes the configured root"))?;
        let path = lexical
            .canonicalize()
            .map_err(|error| FileError::from_io(error, &lexical))?;
        if !path.starts_with(&self.root) {
            return Err(denied(
                "Typst path escapes the configured root through a link",
            ));
        }
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

impl FileResolver for SafeFsResolver {
    fn resolve_binary(&self, id: FileId) -> FileResult<Cow<'_, Bytes>> {
        self.resolve_bytes(id).map(Bytes::new).map(Cow::Owned)
    }

    fn resolve_source(&self, id: FileId) -> FileResult<Cow<'_, Source>> {
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
