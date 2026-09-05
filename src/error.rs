use std::path::PathBuf;

/// A `ctypst` policy, compilation, or export failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A virtual path is not valid in Typst's project namespace.
    #[error("invalid virtual Typst path: {0}")]
    InvalidVirtualPath(String),
    /// The configured filesystem root could not be canonicalized.
    #[error("filesystem root does not exist or cannot be resolved: {path}")]
    InvalidRoot {
        /// Requested filesystem root.
        path: PathBuf,
        #[source]
        /// Canonicalization failure.
        source: std::io::Error,
    },
    /// No supplied bytes contained a valid font face.
    #[error("no valid embedded font was provided")]
    NoValidFont,
    /// One supplied font file contained no valid font face.
    #[error("embedded font file {index} contains no valid font face")]
    InvalidFont {
        /// Zero-based position of the invalid font file.
        index: usize,
    },
    /// A configured finite resource limit was exceeded.
    #[error("{resource} exceeds the {limit_name} limit: {actual} > {maximum}")]
    Limit {
        /// Resource that was rejected.
        resource: String,
        /// Stable name of the enforced limit.
        limit_name: &'static str,
        /// Observed resource size or count.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// Typst could not compile the requested document.
    #[error("Typst compilation failed for {document}: {diagnostics}")]
    Compile {
        /// Requested source path.
        document: String,
        /// Human-readable Typst diagnostics.
        diagnostics: String,
    },
    /// Typst emitted warnings under the deny-warnings policy.
    #[error("Typst emitted warnings for {document}: {diagnostics}")]
    Warnings {
        /// Requested source path.
        document: String,
        /// Human-readable Typst diagnostics.
        diagnostics: String,
    },
    /// The rendered page count violated its contract.
    #[error("{document} rendered {actual} pages; expected {expected}")]
    PageCount {
        /// Requested source path.
        document: String,
        /// Rendered page count.
        actual: usize,
        /// Human-readable expected count or range.
        expected: String,
    },
    /// A synchronization primitive was poisoned by a panic.
    #[error("the embedded Typst engine lock is poisoned")]
    Poisoned,
    /// Metadata queries require a non-empty label.
    #[error("Typst query label is empty")]
    EmptyLabel,
    /// Typst metadata could not be serialized as JSON.
    #[error("Typst metadata query failed: {0}")]
    Query(String),
    /// A measurement request or response failed validation.
    #[error("measurement failed: {0}")]
    Measure(String),
    #[cfg(feature = "pdf")]
    /// The requested deterministic PDF timestamp is outside Typst's range.
    #[error("invalid deterministic PDF timestamp {0}")]
    Timestamp(i64),
    #[cfg(feature = "pdf")]
    /// Typst failed to export a PDF.
    #[error("Typst PDF export failed: {0}")]
    Pdf(String),
    #[cfg(feature = "raster")]
    /// A zero-based raster page index was unavailable.
    #[error("page {page} is unavailable; document has {pages} pages")]
    MissingPage {
        /// Requested zero-based page index.
        page: usize,
        /// Number of pages in the document.
        pages: usize,
    },
    #[cfg(feature = "raster")]
    /// Typst produced an invalid or oversized raster buffer.
    #[error("Typst raster export failed: {0}")]
    Raster(String),
    #[cfg(feature = "format")]
    /// Typstyle failed to format source text.
    #[error("Typst source formatting failed: {0}")]
    Format(String),
}

/// Result type used by `ctypst`.
pub type Result<T> = std::result::Result<T, Error>;
