//! Python bindings for `ctypst`: measurement, compilation, and export.
//!
//! Thin over the Rust API with identical semantics. Errors surface as
//! `ValueError` for invalid requests and `RuntimeError` for engine
//! failures.

use std::collections::HashMap;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::{IntoPyObjectExt, Py, PyAny};

fn map_error(fault: &ctypst::Error) -> PyErr {
    let message = fault.to_string();
    match fault {
        ctypst::Error::Measure(_)
        | ctypst::Error::InvalidVirtualPath(_)
        | ctypst::Error::InvalidRoot { .. }
        | ctypst::Error::InvalidFont { .. }
        | ctypst::Error::NoValidFont
        | ctypst::Error::Limit { .. } => PyValueError::new_err(message),
        _ => PyRuntimeError::new_err(message),
    }
}

/// Text weight carried by a measurement item.
#[pyclass(eq, eq_int, frozen, hash, from_py_object)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Weight {
    Regular,
    Bold,
}

#[pymethods]
impl Weight {
    #[must_use]
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Bold => "bold",
        }
    }

    // By value would be idiomatic (Weight is Copy), but pymethods require
    // shared receivers.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn __repr__(&self) -> &'static str {
        match self {
            Self::Regular => "Weight.Regular",
            Self::Bold => "Weight.Bold",
        }
    }
}

fn parse_weight(value: &Bound<'_, PyAny>) -> PyResult<ctypst::measure::Weight> {
    if let Ok(weight) = value.extract::<Weight>() {
        return Ok(match weight {
            Weight::Regular => ctypst::measure::Weight::Regular,
            Weight::Bold => ctypst::measure::Weight::Bold,
        });
    }
    if let Ok(name) = value.extract::<String>() {
        return match name.as_str() {
            "regular" => Ok(ctypst::measure::Weight::Regular),
            "bold" => Ok(ctypst::measure::Weight::Bold),
            _ => Err(PyValueError::new_err(format!("unknown weight {name:?}"))),
        };
    }
    Err(PyValueError::new_err(
        "weight must be Weight or 'regular'/'bold'",
    ))
}

/// One fragment to measure: opaque id, raw text, style, available width.
#[pyclass(get_all, set_all, from_py_object)]
#[derive(Clone, Debug)]
pub struct MeasureItem {
    pub id: String,
    pub text: String,
    pub font_size: f64,
    pub weight: Weight,
    pub usable_width_pt: f64,
}

#[pymethods]
impl MeasureItem {
    #[new]
    #[pyo3(signature = (id, text, font_size=10.5, weight=None, usable_width_pt=400.0))]
    fn construct(
        id: String,
        text: String,
        font_size: f64,
        weight: Option<&Bound<'_, PyAny>>,
        usable_width_pt: f64,
    ) -> PyResult<Self> {
        let weight = weight
            .as_ref()
            .map_or(
                Ok::<Weight, PyErr>(Weight::Regular),
                |value| match parse_weight(value)? {
                    ctypst::measure::Weight::Regular => Ok(Weight::Regular),
                    ctypst::measure::Weight::Bold => Ok(Weight::Bold),
                },
            )?;
        Ok(Self {
            id,
            text,
            font_size,
            weight,
            usable_width_pt,
        })
    }
}

fn convert_item(item: &MeasureItem) -> ctypst::measure::MeasureItem {
    ctypst::measure::MeasureItem {
        id: item.id.clone(),
        text: item.text.clone(),
        font_size: item.font_size,
        weight: match item.weight {
            Weight::Regular => ctypst::measure::Weight::Regular,
            Weight::Bold => ctypst::measure::Weight::Bold,
        },
        usable_width_pt: item.usable_width_pt,
    }
}

/// Measurement format: shared page and font geometry.
#[pyclass(get_all, set_all, from_py_object)]
#[derive(Clone, Debug)]
pub struct MeasureFormat {
    pub font: String,
    pub base_font_size: f64,
    pub entry_heading_size: f64,
    pub leading_value: f64,
    pub leading_relative: bool,
    pub margin_left: f64,
    pub margin_right: f64,
    pub page_size: String,
}

#[pymethods]
impl MeasureFormat {
    #[new]
    #[pyo3(signature = (font=None, base_font_size=10.5, entry_heading_size=11.0, leading_value=0.6, leading_relative=true, margin_left=15.0, margin_right=15.0, page_size=None))]
    #[allow(clippy::too_many_arguments)]
    fn construct(
        font: Option<String>,
        base_font_size: f64,
        entry_heading_size: f64,
        leading_value: f64,
        leading_relative: bool,
        margin_left: f64,
        margin_right: f64,
        page_size: Option<String>,
    ) -> Self {
        Self {
            font: font.unwrap_or_else(|| "Archivo".to_owned()),
            base_font_size,
            entry_heading_size,
            leading_value,
            leading_relative,
            margin_left,
            margin_right,
            page_size: page_size.unwrap_or_else(|| "a4".to_owned()),
        }
    }

    #[must_use]
    pub fn leading_em(&self) -> f64 {
        ctypst::measure::leading_em(
            self.leading_value,
            self.leading_relative,
            self.base_font_size,
        )
    }
}

fn convert_format(format: &MeasureFormat) -> ctypst::measure::MeasureFormat {
    ctypst::measure::MeasureFormat {
        font: format.font.clone(),
        base_font_size: format.base_font_size,
        entry_heading_size: format.entry_heading_size,
        leading_value: format.leading_value,
        leading_relative: format.leading_relative,
        margin_left: format.margin_left,
        margin_right: format.margin_right,
        page_size: format.page_size.clone(),
    }
}

/// Calibration ratios from the four Typst probes. Observability only.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone, Copy, Debug)]
pub struct MeasureCalibration {
    pub cap_ratio_regular: f64,
    pub advance_ratio_regular: f64,
    pub cap_ratio_bold: f64,
    pub advance_ratio_bold: f64,
}

/// One measured fragment.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct MeasureResult {
    pub id: String,
    pub width_pt: f64,
    pub height_pt: f64,
    pub lines: u64,
    pub char_budget: Option<i64>,
}

/// Native measurement client with result caching.
#[pyclass(skip_from_py_object)]
pub struct MeasureClient {
    inner: ctypst::measure::MeasureClient,
}

#[pymethods]
impl MeasureClient {
    #[new]
    #[pyo3(signature = (fonts=None))]
    fn construct(fonts: Option<Vec<Vec<u8>>>) -> PyResult<Self> {
        let fonts = fonts.unwrap_or_else(|| {
            ctypst::fonts::documents()
                .iter()
                .map(|face| face.to_vec())
                .collect()
        });
        Ok(Self {
            inner: ctypst::measure::MeasureClient::new(fonts).map_err(|fault| map_error(&fault))?,
        })
    }

    // Vec, not a slice: pyo3 function arguments cannot borrow sequences.
    #[allow(clippy::needless_pass_by_value)]
    fn measure_all(
        &mut self,
        format: &MeasureFormat,
        items: Vec<PyRef<'_, MeasureItem>>,
    ) -> PyResult<Vec<MeasureResult>> {
        let owned_format = convert_format(format);
        let owned_items = items
            .iter()
            .map(|item| convert_item(item))
            .collect::<Vec<_>>();
        let results = self
            .inner
            .measure_all(&owned_format, &owned_items)
            .map_err(|fault| map_error(&fault))?;
        Ok(results
            .into_iter()
            .map(|value| MeasureResult {
                id: value.id,
                width_pt: value.width_pt,
                height_pt: value.height_pt,
                lines: value.lines,
                char_budget: value.char_budget,
            })
            .collect())
    }

    fn calibration(&self) -> Option<MeasureCalibration> {
        self.inner.calibration().map(|value| MeasureCalibration {
            cap_ratio_regular: value.cap_ratio_regular,
            advance_ratio_regular: value.advance_ratio_regular,
            cap_ratio_bold: value.cap_ratio_bold,
            advance_ratio_bold: value.advance_ratio_bold,
        })
    }

    fn compile_count(&self) -> u64 {
        self.inner.compile_count()
    }
}

/// One compiled document: rendered many times without recompiling.
#[pyclass(skip_from_py_object)]
pub struct Document {
    engine: std::sync::Arc<ctypst::Engine>,
    document: ctypst::Document,
}

#[pymethods]
impl Document {
    fn page_count(&self) -> usize {
        self.document.pages().len()
    }

    fn svg_page(&self, page: usize) -> PyResult<String> {
        self.engine
            .svg_page(&self.document, page)
            .map_err(|fault| map_error(&fault))
    }

    #[pyo3(signature = (epoch=0))]
    fn pdf(&self, epoch: i64) -> PyResult<Vec<u8>> {
        self.engine
            .pdf(&self.document, epoch)
            .map_err(|fault| map_error(&fault))
    }

    fn query(&self, py: Python<'_>, label: &str) -> PyResult<Vec<Py<PyAny>>> {
        let values =
            ctypst::query_json(&self.document, label).map_err(|fault| map_error(&fault))?;
        values.iter().map(|value| convert_json(py, value)).collect()
    }
}

fn convert_json(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => py.None().into_py_any(py),
        serde_json::Value::Bool(flag) => flag.into_py_any(py),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int.into_py_any(py)
            } else if let Some(uint) = number.as_u64() {
                uint.into_py_any(py)
            } else if let Some(float) = number.as_f64() {
                float.into_py_any(py)
            } else {
                Err(PyRuntimeError::new_err("number is not finite"))
            }
        }
        serde_json::Value::String(text) => text.as_str().into_py_any(py),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| convert_json(py, item))
            .collect::<PyResult<Vec<Py<PyAny>>>>()?
            .into_py_any(py),
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key, convert_json(py, item)?)?;
            }
            dict.into_py_any(py)
        }
    }
}

/// Reusable embedded Typst compiler with no ambient network or font access.
#[pyclass(skip_from_py_object)]
pub struct Engine {
    inner: std::sync::Arc<ctypst::Engine>,
}

#[pymethods]
impl Engine {
    #[new]
    #[pyo3(signature = (fonts=None))]
    fn construct(fonts: Option<Vec<Vec<u8>>>) -> PyResult<Self> {
        let inner = match fonts {
            Some(faces) => ctypst::Engine::builder().fonts(faces),
            None => ctypst::Engine::builder().fonts(ctypst::fonts::documents()),
        }
        .build()
        .map_err(|fault| map_error(&fault))?;
        Ok(Self {
            inner: std::sync::Arc::new(inner),
        })
    }

    #[pyo3(signature = (source, inputs=None, sources=None, binaries=None))]
    #[allow(clippy::too_many_arguments)]
    fn compile(
        &self,
        source: String,
        inputs: Option<HashMap<String, String>>,
        sources: Option<HashMap<String, String>>,
        binaries: Option<HashMap<String, Vec<u8>>>,
    ) -> PyResult<Document> {
        let mut request =
            ctypst::CompileRequest::new("main.typ".to_owned()).source_file("main.typ", source);
        if let Some(inputs) = inputs {
            request = request.inputs(inputs.into_iter().collect());
        }
        if let Some(sources) = sources {
            for (path, content) in sources {
                request = request.source_file(path, content);
            }
        }
        if let Some(binaries) = binaries {
            for (path, content) in binaries {
                request = request.binary_file(path, content);
            }
        }
        let engine = std::sync::Arc::clone(&self.inner);
        let document = engine
            .compile(request)
            .map(|output| output.document)
            .map_err(|fault| map_error(&fault))?;
        Ok(Document {
            engine: self.inner.clone(),
            document,
        })
    }
}

/// Fragment-level character budget: positive room left, negative overflow,
/// `None` when the fragment is empty or has no measurable width.
#[pyfunction]
fn char_budget(text: &str, width_pt: f64, usable_width_pt: f64) -> Option<i64> {
    ctypst::measure::char_budget(text, width_pt, usable_width_pt)
}

/// Canonical em multiplier for a leading value.
#[pyfunction]
fn leading_em(leading_value: f64, leading_relative: bool, base_font_size: f64) -> f64 {
    ctypst::measure::leading_em(leading_value, leading_relative, base_font_size)
}

/// The ctypst Python API: measurement, compilation, and export.
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Weight>()?;
    m.add_class::<MeasureItem>()?;
    m.add_class::<MeasureFormat>()?;
    m.add_class::<MeasureCalibration>()?;
    m.add_class::<MeasureResult>()?;
    m.add_class::<MeasureClient>()?;
    m.add_class::<Engine>()?;
    m.add_class::<Document>()?;
    m.add_function(wrap_pyfunction!(char_budget, m)?)?;
    m.add_function(wrap_pyfunction!(leading_em, m)?)?;
    m.add("PROTOCOL_VERSION", ctypst::measure::PROTOCOL_VERSION)?;
    m.add("QUERY_LABEL", ctypst::measure::QUERY_LABEL)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
