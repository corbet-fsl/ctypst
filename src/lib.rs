#![forbid(unsafe_code)]
//! Safe, deterministic, embedded Typst for Rust applications.

mod config;
mod engine;
mod error;
mod files;

#[cfg(feature = "document-fonts")]
pub mod fonts;
#[cfg(feature = "format")]
mod format;
#[cfg(feature = "measure")]
pub mod measure;
#[cfg(feature = "pdf")]
mod pdf;
mod query;
#[cfg(feature = "raster")]
mod raster;

pub use config::{CompileOutput, CompileRequest, DiagnosticsPolicy, Limits, PageConstraint};
pub use engine::{Engine, EngineBuilder};
pub use error::{Error, Result};
#[cfg(feature = "format")]
pub use format::format_source;
pub use query::query_json;
#[cfg(feature = "raster")]
pub use raster::RasterPage;
pub use typst_layout::PagedDocument as Document;
