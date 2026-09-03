use std::collections::BTreeMap;

use crate::Document;

/// Finite resource limits applied to every engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum UTF-8 byte length of a virtual or requested path.
    pub max_path_bytes: usize,
    /// Maximum number of virtual or filesystem files in one namespace or compile.
    pub max_files: usize,
    /// Maximum bytes read from or supplied for one file.
    pub max_file_bytes: usize,
    /// Maximum aggregate bytes across virtual files or one filesystem compile.
    pub max_total_bytes: usize,
    /// Maximum number of supplied font files and parsed font faces.
    pub max_fonts: usize,
    /// Maximum bytes in one supplied font file.
    pub max_font_bytes: usize,
    /// Maximum aggregate bytes across supplied font files.
    pub max_total_font_bytes: usize,
    /// Maximum number of Typst system inputs in one compile request.
    pub max_inputs: usize,
    /// Maximum aggregate UTF-8 bytes across input keys and values.
    pub max_input_bytes: usize,
    /// Maximum accepted pages in a compiled document.
    pub max_pages: usize,
    /// Maximum bytes in one exported PDF.
    pub max_pdf_bytes: usize,
    /// Maximum pixels in one rasterized page.
    pub max_raster_pixels: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_path_bytes: 4 * 1024,
            max_files: 256,
            max_file_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_fonts: 64,
            max_font_bytes: 16 * 1024 * 1024,
            max_total_font_bytes: 64 * 1024 * 1024,
            max_inputs: 256,
            max_input_bytes: 2 * 1024 * 1024,
            max_pages: 32,
            max_pdf_bytes: 64 * 1024 * 1024,
            max_raster_pixels: 20_000_000,
        }
    }
}

/// How compilation warnings affect success.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiagnosticsPolicy {
    /// Treat every Typst warning as a failed build.
    #[default]
    DenyWarnings,
    /// Return warnings to the caller without failing.
    AllowWarnings,
}

/// Page-count requirement for one compilation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageConstraint {
    /// Require a non-empty document within the engine-wide maximum.
    #[default]
    Any,
    /// Require exactly this many pages.
    Exactly(usize),
    /// Require an inclusive page range.
    Between {
        /// Smallest accepted page count.
        minimum: usize,
        /// Largest accepted page count.
        maximum: usize,
    },
}

impl PageConstraint {
    pub(crate) fn accepts(self, actual: usize, hard_maximum: usize) -> bool {
        if actual == 0 || actual > hard_maximum {
            return false;
        }
        match self {
            Self::Any => true,
            Self::Exactly(expected) => actual == expected,
            Self::Between { minimum, maximum } => (minimum..=maximum).contains(&actual),
        }
    }

    pub(crate) fn describe(self, hard_maximum: usize) -> String {
        match self {
            Self::Any => format!("1-{hard_maximum}"),
            Self::Exactly(expected) => expected.to_string(),
            Self::Between { minimum, maximum } => format!("{minimum}-{maximum}"),
        }
    }
}

/// One compilation request.
#[derive(Clone, Debug)]
pub struct CompileRequest {
    pub(crate) source: String,
    pub(crate) inputs: BTreeMap<String, String>,
    pub(crate) sources: BTreeMap<String, String>,
    pub(crate) binaries: BTreeMap<String, Vec<u8>>,
    pub(crate) pages: PageConstraint,
    pub(crate) diagnostics: DiagnosticsPolicy,
}

impl CompileRequest {
    /// Create a request for a virtual or root-relative Typst source path.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            inputs: BTreeMap::new(),
            sources: BTreeMap::new(),
            binaries: BTreeMap::new(),
            pages: PageConstraint::Any,
            diagnostics: DiagnosticsPolicy::DenyWarnings,
        }
    }

    /// Add one Typst system input.
    #[must_use]
    pub fn input(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inputs.insert(key.into(), value.into());
        self
    }

    /// Replace all Typst system inputs on this request.
    #[must_use]
    pub fn inputs(mut self, inputs: BTreeMap<String, String>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Overlay a virtual Typst source for this compilation only.
    #[must_use]
    pub fn source_file(mut self, path: impl Into<String>, source: impl Into<String>) -> Self {
        self.sources.insert(path.into(), source.into());
        self
    }

    /// Overlay a virtual binary asset for this compilation only.
    #[must_use]
    pub fn binary_file(mut self, path: impl Into<String>, binary: impl Into<Vec<u8>>) -> Self {
        self.binaries.insert(path.into(), binary.into());
        self
    }

    /// Apply a page-count contract to the compiled document.
    #[must_use]
    pub const fn pages(mut self, pages: PageConstraint) -> Self {
        self.pages = pages;
        self
    }

    /// Select how Typst warnings affect this compilation.
    #[must_use]
    pub const fn diagnostics(mut self, diagnostics: DiagnosticsPolicy) -> Self {
        self.diagnostics = diagnostics;
        self
    }
}

/// Successful Typst output and any explicitly accepted warnings.
pub struct CompileOutput {
    /// Compiled paged Typst document.
    pub document: Document,
    /// Warnings retained when the request explicitly allows them.
    pub warnings: Vec<String>,
}
