use crate::{Document, Engine, Error, Result};

/// One Typst page rendered as tightly packed RGBA8 pixels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterPage {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Tightly packed row-major RGBA8 bytes.
    pub pixels: Vec<u8>,
}

impl Engine {
    /// Rasterize one zero-based page with Typst's native renderer.
    pub fn rasterize(&self, document: &Document, page: usize) -> Result<RasterPage> {
        let page_ref = document.pages().get(page).ok_or(Error::MissingPage {
            page,
            pages: document.pages().len(),
        })?;
        let size = page_ref.frame.size();
        let width = pixel_dimension(size.x.to_pt())?;
        let height = pixel_dimension(size.y.to_pt())?;
        self.check_raster_size(width.saturating_mul(height))?;
        let pixmap = typst_render::render(page_ref, &typst_render::RenderOptions::default());
        let actual_pixels = usize::try_from(pixmap.width())
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(pixmap.height()).unwrap_or(usize::MAX));
        self.check_raster_size(actual_pixels)?;
        let expected_bytes = actual_pixels.checked_mul(4).ok_or_else(|| {
            Error::Raster("RGBA buffer size overflows the platform integer".to_owned())
        })?;
        if pixmap.data().len() != expected_bytes {
            return Err(Error::Raster(format!(
                "RGBA buffer has {} bytes; expected {expected_bytes}",
                pixmap.data().len()
            )));
        }
        Ok(RasterPage {
            width: pixmap.width(),
            height: pixmap.height(),
            pixels: pixmap.data().to_vec(),
        })
    }
}

fn pixel_dimension(points: f64) -> Result<usize> {
    let pixels = (2.0 * points).round().max(1.0);
    if !pixels.is_finite() || pixels > f64::from(u32::MAX) {
        return Err(Error::Limit {
            resource: "raster dimension".to_owned(),
            limit_name: "u32 pixels",
            actual: usize::MAX,
            maximum: u32::MAX as usize,
        });
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(pixels as usize)
}
