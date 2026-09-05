//! Deterministic per-page SVG export.
use crate::{Document, Error, Result};

/// Render one zero-based page to an SVG document string.
impl crate::Engine {
    /// Render one zero-based page of an already compiled document.
    ///
    /// # Errors
    ///
    /// Returns an error when the page index is out of range.
    pub fn svg_page(&self, document: &Document, page: usize) -> Result<String> {
        let frame = document.pages().get(page).ok_or(Error::MissingPage {
            page,
            pages: document.pages().len(),
        })?;
        Ok(typst_svg::svg(
            frame,
            &typst_svg::SvgOptions {
                render_bleed: false,
                pretty: false,
            },
        ))
    }
}
