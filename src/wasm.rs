//! WebAssembly boundary: the whole `ctypst` runtime for browsers and Node.
//!
//! This module only builds with the `wasm` feature. It exposes the engine,
//! the versioned measurement client, and the exporters over JSON values so
//! JavaScript callers cross exactly one boundary type. Fonts ship embedded;
//! callers configure nothing.
//!
//! [`CompiledDoc`] compiles once and renders many times: callers rendering
//! several pages of one document pay exactly one compilation.

use std::collections::BTreeMap;
use std::sync::Arc;

use wasm_bindgen::prelude::*;

use crate::measure::{MeasureClient, MeasureFormat, MeasureItem};
use crate::{CompileRequest, Engine};

fn error(message: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&message.to_string())
}

fn parse_map(raw: &str) -> Result<BTreeMap<String, String>, JsValue> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|fault| error(format!("inputs are not JSON: {fault}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| error("inputs must be a JSON object"))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|text| (key.clone(), text.to_owned()))
                .ok_or_else(|| error("input values must be strings"))
        })
        .collect()
}

/// One `ctypst` runtime: embedded fonts, measurement, compilation, export.
#[wasm_bindgen]
pub struct Ctypst {
    engine: Arc<Engine>,
    overlays: Vec<(String, Overlay)>,
    measure: MeasureClient,
}

enum Overlay {
    Source(String),
    Binary(Vec<u8>),
}

#[wasm_bindgen]
impl Ctypst {
    /// Open a runtime on the embedded document font pack.
    ///
    /// # Errors
    ///
    /// JavaScript sees a string error when no embedded font parses.
    #[wasm_bindgen(constructor)]
    pub fn open() -> Result<Ctypst, JsValue> {
        let engine = Arc::new(
            Engine::builder()
                .fonts(crate::fonts::documents())
                .build()
                .map_err(|fault| error(format!("cannot open ctypst: {fault}")))?,
        );
        let measure = MeasureClient::new(crate::fonts::documents())
            .map_err(|fault| error(format!("cannot open measurement: {fault}")))?;
        Ok(Self {
            engine,
            overlays: Vec::new(),
            measure,
        })
    }

    /// Crate and protocol versions for cache keys and diagnostics.
    #[must_use]
    pub fn versions() -> String {
        serde_json::json!({
            "ctypst": env!("CARGO_PKG_VERSION"),
            "protocol": crate::measure::PROTOCOL_VERSION,
        })
        .to_string()
    }

    /// Pin a virtual text source visible to every later compilation.
    ///
    /// Replaces any overlay already pinned at the same path, so repeated
    /// asset uploads stay bounded.
    pub fn add_source(&mut self, path: String, content: String) {
        self.replace_overlay(path, Overlay::Source(content));
    }

    /// Pin a virtual binary asset visible to every later compilation.
    ///
    /// Replaces any overlay already pinned at the same path.
    pub fn add_binary(&mut self, path: String, content: Vec<u8>) {
        self.replace_overlay(path, Overlay::Binary(content));
    }

    fn replace_overlay(&mut self, path: String, overlay: Overlay) {
        if let Some(slot) = self.overlays.iter_mut().find(|(pinned, _)| *pinned == path) {
            slot.1 = overlay;
        } else {
            self.overlays.push((path, overlay));
        }
    }

    /// Measure a JSON array of items with a JSON format object.
    ///
    /// Returns `{results, calibration}` or a string error. Never throws.
    pub fn measure_all(&mut self, format_json: &str, items_json: &str) -> Result<String, JsValue> {
        let format: MeasureFormat = serde_json::from_str(format_json)
            .map_err(|fault| error(format!("format is not a measure format: {fault}")))?;
        let items: Vec<MeasureItem> = serde_json::from_str(items_json)
            .map_err(|fault| error(format!("items are not measure items: {fault}")))?;
        let results = self
            .measure
            .measure_all(&format, &items)
            .map_err(|fault| error(format!("measurement failed: {fault}")))?;
        let calibration = self.measure.calibration();
        serde_json::to_string(&serde_json::json!({
            "results": results,
            "calibration": calibration,
        }))
        .map_err(|fault| error(format!("cannot encode results: {fault}")))
    }

    /// Compile a source string with JSON inputs into a reusable document.
    pub fn compile(&self, source: &str, inputs_json: &str) -> Result<CompiledDoc, JsValue> {
        let inputs = parse_map(inputs_json)?;
        let mut request =
            CompileRequest::new("main.typ".to_owned()).source_file("main.typ", source.to_owned());
        for (path, overlay) in &self.overlays {
            request = match overlay {
                Overlay::Source(content) => request.source_file(path.clone(), content.clone()),
                Overlay::Binary(bytes) => request.binary_file(path.clone(), bytes.clone()),
            };
        }
        request = request.inputs(inputs);
        let document = self
            .engine
            .compile(request)
            .map(|output| output.document)
            .map_err(|fault| error(format!("compilation failed: {fault}")))?;
        Ok(CompiledDoc {
            engine: Arc::clone(&self.engine),
            document,
        })
    }
}

/// One compiled document: rendered many times without recompiling.
#[wasm_bindgen]
pub struct CompiledDoc {
    engine: Arc<Engine>,
    document: crate::Document,
}

#[wasm_bindgen]
impl CompiledDoc {
    /// Page count of the compiled document.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.document.pages().len()
    }

    /// Render one zero-based page to SVG text.
    pub fn svg_page(&self, page: usize) -> Result<String, JsValue> {
        self.engine
            .svg_page(&self.document, page)
            .map_err(|fault| error(format!("SVG export failed: {fault}")))
    }

    /// Render all pages into one merged SVG document with `gap_pt` between pages.
    pub fn svg_merged(&self, gap_pt: f64) -> Result<String, JsValue> {
        if !gap_pt.is_finite() || gap_pt < 0.0 {
            return Err(error("SVG gap must be finite and non-negative"));
        }
        Ok(typst_svg::svg_merged(
            &self.document,
            &typst_svg::SvgOptions {
                render_bleed: false,
                pretty: false,
            },
            typst::layout::Abs::pt(gap_pt),
        ))
    }

    /// Export a deterministic PDF (epoch 0).
    pub fn pdf(&self) -> Result<Vec<u8>, JsValue> {
        self.engine
            .pdf(&self.document, 0)
            .map_err(|fault| error(format!("PDF export failed: {fault}")))
    }
}

#[cfg(test)]
mod tests {
    use super::Ctypst;

    #[test]
    fn repeated_overlays_replace_instead_of_accumulating() {
        let mut runtime = Ctypst::open().expect("runtime opens on embedded fonts");
        runtime.add_source("/x.typ".to_owned(), "Version one".to_owned());
        runtime.add_source("/x.typ".to_owned(), "Version two".to_owned());
        runtime.add_binary("/x.bin".to_owned(), vec![1, 2, 3]);
        runtime.add_binary("/x.bin".to_owned(), vec![4, 5]);
        let document = runtime
            .compile("#include \"/x.typ\"", "{}")
            .expect("overlay include compiles");
        assert_eq!(document.page_count(), 1);
    }
}
