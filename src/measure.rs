//! Typed native adapter for the versioned measurement protocol.
//!
//! [`MeasureClient`] compiles [`PROTOCOL_VERSION`] requests against the one
//! shared Typst measurement program (`typst/measure-v1.typ`). Callers pass
//! product records as data; no caller builds measurement source, repeats
//! calibration, derives lines, or defines cache keys. Results cache by
//! exact request so interactive callers recompile only on misses, and
//! every request weakness fails loudly instead of degrading silently.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{CompileRequest, Engine, Error, Result, query_json};

/// Versioned measurement contract served by this adapter.
pub const PROTOCOL_VERSION: &str = "ctypst-measure-v1";

/// Metadata label carrying every measurement result (bare name; the
/// program attaches it as `<ctypst-measure-v1>`).
pub const QUERY_LABEL: &str = "ctypst-measure-v1";

/// Virtual path of the shared measurement program.
const PROGRAM_PATH: &str = "/ctypst/measure-v1.typ";

/// Virtual path of the per-compile JSON request.
///
/// Stored as a binary overlay: `json()` reads through file bytes, not
/// parsed sources.
const REQUEST_PATH: &str = "/ctypst/request.json";

/// Embedded shared measurement program.
const PROGRAM_SOURCE: &str = include_str!("../typst/measure-v1.typ");

/// Response id carrying the calibration record.
const CALIBRATION_ID: &str = "__calibration";

/// FNV-1a hash of the embedded measurement program.
///
/// Part of every cache key so a program change can never serve stale
/// results. Deterministic across processes (unlike the default hasher).
const ASSET_HASH: u64 = fnv1a64(PROGRAM_SOURCE.as_bytes());

/// Text weight carried by a measurement item.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Weight {
    /// Regular text weight.
    Regular,
    /// Bold text weight.
    Bold,
}

/// One fragment to measure: opaque id, raw text, style, available width.
///
/// Text travels verbatim; the measurement program owns escaping and the
/// supported `*`/`_` markup policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasureItem {
    /// Opaque caller id, echoed back on the result. Must be unique per call.
    pub id: String,
    /// Raw fragment text, possibly empty.
    pub text: String,
    /// Font size in points. Must be finite and positive.
    pub font_size: f64,
    /// Text weight.
    pub weight: Weight,
    /// Available wrap width in points. Must be finite and positive.
    pub usable_width_pt: f64,
}

/// Measurement format: the shared page and font geometry.
///
/// Hosts map their product format onto this shape; relative leadings may
/// stay relative because the adapter applies the canonical rule.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasureFormat {
    /// Font family name, for example `"Archivo"`. Empty falls back to Archivo.
    pub font: String,
    /// Base font size in points. Must be finite and positive.
    pub base_font_size: f64,
    /// Entry heading size in points (bold calibration). Must be finite and positive.
    pub entry_heading_size: f64,
    /// Leading value: em multiplier when `leading_relative`, points otherwise.
    pub leading_value: f64,
    /// Whether `leading_value` is already an em multiplier.
    pub leading_relative: bool,
    /// Left page margin in millimetres. Must be finite and non-negative.
    pub margin_left: f64,
    /// Right page margin in millimetres. Must be finite and non-negative.
    pub margin_right: f64,
    /// Page size name; `"us-letter"` selects 215.9mm, anything else is A4 width.
    pub page_size: String,
}

/// Calibration ratios from the four Typst probes.
///
/// Observability only: line derivation already ran inside the program.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasureCalibration {
    /// Cap-height ratio for regular text.
    pub cap_ratio_regular: f64,
    /// Line-advance ratio for regular text.
    pub advance_ratio_regular: f64,
    /// Cap-height ratio for bold text.
    pub cap_ratio_bold: f64,
    /// Line-advance ratio for bold text.
    pub advance_ratio_bold: f64,
}

/// One measured fragment: exact Typst facts plus the host-side budget.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasureResult {
    /// Echoed opaque caller id.
    pub id: String,
    /// Natural (unwrapped) width in points.
    pub width_pt: f64,
    /// Wrapped height in points.
    pub height_pt: f64,
    /// Derived integer line count, at least one.
    pub lines: u64,
    /// Character budget: positive room left, negative overflow, none when
    /// the fragment is empty or has no measurable width.
    pub char_budget: Option<i64>,
}

/// Native measurement client with result caching.
///
/// The engine is owned here so interactive callers pay one compile per
/// cache miss batch and zero compiles on full hits or empty requests.
pub struct MeasureClient {
    engine: Engine,
    font_hash: u64,
    tag: Option<String>,
    results: HashMap<String, MeasureResult>,
    calibration: Option<MeasureCalibration>,
    compiles: u64,
}

impl MeasureClient {
    /// Build a client around one reusable embedded Typst engine.
    ///
    /// # Errors
    ///
    /// Returns an error when no supplied font data contains a valid font.
    #[must_use = "a measurement client does nothing until it measures"]
    pub fn new(fonts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> Result<Self> {
        let collected: Vec<Vec<u8>> = fonts
            .into_iter()
            .map(|font| font.as_ref().to_vec())
            .collect();
        let mut font_hash = 0xcbf2_9ce4_8422_2325_u64;
        for bytes in &collected {
            font_hash = fnv1a64_update(font_hash, bytes);
        }
        let engine = Engine::builder()
            .fonts(collected.iter().map(Vec::as_slice))
            .source(PROGRAM_PATH, PROGRAM_SOURCE)
            .map_err(|error| Error::Measure(format!("cannot install {PROTOCOL_VERSION}: {error}")))?
            .build()?;
        Ok(Self {
            engine,
            font_hash,
            tag: None,
            results: HashMap::new(),
            calibration: None,
            compiles: 0,
        })
    }

    /// Measure every item, compiling once per cache-miss batch.
    ///
    /// Results follow request order. An empty item list returns empty
    /// without compiling.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is invalid (empty or duplicate
    /// ids, non-finite or non-positive dimensions), when Typst cannot
    /// run, or when any result is missing, duplicated, unexpected, or
    /// out of range.
    pub fn measure_all(
        &mut self,
        format: &MeasureFormat,
        items: &[MeasureItem],
    ) -> Result<Vec<MeasureResult>> {
        validate(format, items)?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let tag = format_tag(self.font_hash, format);
        if self.tag.as_deref() != Some(&tag) {
            self.results.clear();
            self.calibration = None;
            self.tag = Some(tag);
        }
        let misses: Vec<MeasureItem> = items
            .iter()
            .filter(|item| {
                !self
                    .results
                    .contains_key(&item_key(self.font_hash, format, item))
            })
            .cloned()
            .collect();
        if !misses.is_empty() {
            self.compile_misses(format, &misses)?;
        }
        items
            .iter()
            .map(|item| {
                self.results
                    .get(&item_key(self.font_hash, format, item))
                    .cloned()
                    .ok_or_else(|| Error::Measure(format!("cache omitted fragment {}", item.id)))
            })
            .collect()
    }

    /// Calibration ratios from the latest compile, if any ran.
    #[must_use]
    pub fn calibration(&self) -> Option<MeasureCalibration> {
        self.calibration
    }

    /// Engine compiles performed so far. Guards the batching contract:
    /// one compile per miss batch, zero on full hits or empty requests.
    #[must_use]
    pub fn compile_count(&self) -> u64 {
        self.compiles
    }

    fn compile_misses(&mut self, format: &MeasureFormat, misses: &[MeasureItem]) -> Result<()> {
        let request = serde_json::json!({
            "version": PROTOCOL_VERSION,
            "format": normalized_format(format),
            "items": misses,
        });
        let source = serde_json::to_string(&request)
            .map_err(|error| Error::Measure(format!("cannot serialize request: {error}")))?;
        let document = self
            .engine
            .compile(CompileRequest::new(PROGRAM_PATH.to_owned()).binary_file(REQUEST_PATH, source))
            .map_err(|error| Error::Measure(format!("{PROTOCOL_VERSION} run failed: {error}")))?
            .document;
        self.compiles += 1;
        let values = query_json(&document, QUERY_LABEL)
            .map_err(|error| Error::Measure(format!("{PROTOCOL_VERSION} query failed: {error}")))?;
        decode_response(self.font_hash, format, misses, &values, &mut self.results)?;
        let calibration = decode_calibration(&values)?;
        self.calibration = Some(calibration);
        Ok(())
    }
}

/// Fragment-level character budget: positive room left, negative overflow.
///
/// Uses host UTF-16 string semantics, which Typst strings do not provide,
/// so this stays host-side with frozen unit coverage.
#[must_use]
pub fn char_budget(text: &str, width_pt: f64, usable_width_pt: f64) -> Option<i64> {
    if text.is_empty() || width_pt <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let units = text.encode_utf16().count() as f64;
    #[allow(clippy::cast_possible_truncation)]
    let budget = (units * (usable_width_pt / width_pt - 1.0)).round() as i64;
    Some(budget)
}

/// Canonical em multiplier: relative values pass through, point values
/// divide by the base size rounded half away from zero to four decimals.
#[must_use]
pub fn leading_em(leading_value: f64, leading_relative: bool, base_font_size: f64) -> f64 {
    if leading_relative {
        leading_value
    } else {
        (leading_value / base_font_size * 10_000.0).round() / 10_000.0
    }
}

const fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_update(0xcbf2_9ce4_8422_2325_u64, bytes)
}

const fn fnv1a64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
        index += 1;
    }
    hash
}

fn effective_font(format: &MeasureFormat) -> &str {
    if format.font.is_empty() {
        "Archivo"
    } else {
        &format.font
    }
}

fn effective_page_size(format: &MeasureFormat) -> &str {
    if format.page_size.is_empty() {
        "a4"
    } else {
        &format.page_size
    }
}

fn normalized_format(format: &MeasureFormat) -> serde_json::Value {
    serde_json::json!({
        "font": effective_font(format),
        "baseFontSize": format.base_font_size,
        "entryHeadingSize": format.entry_heading_size,
        "leadingEm": leading_em(format.leading_value, format.leading_relative, format.base_font_size),
        "marginLeft": format.margin_left,
        "marginRight": format.margin_right,
        "pageSize": effective_page_size(format),
    })
}

fn format_tag(font_hash: u64, format: &MeasureFormat) -> String {
    format!(
        "{PROTOCOL_VERSION}/{ASSET_HASH:016x}/{}/{font_hash:016x}/{}",
        env!("CARGO_PKG_VERSION"),
        serde_json::to_string(&normalized_format(format)).unwrap_or_default(),
    )
}

fn item_key(font_hash: u64, format: &MeasureFormat, item: &MeasureItem) -> String {
    format!(
        "{}/{}",
        format_tag(font_hash, format),
        serde_json::to_string(item).unwrap_or_default(),
    )
}

fn validate(format: &MeasureFormat, items: &[MeasureItem]) -> Result<()> {
    let fail = |what: &str| Error::Measure(format!("invalid {PROTOCOL_VERSION} request: {what}"));
    for (field, value) in [
        ("base_font_size", format.base_font_size),
        ("entry_heading_size", format.entry_heading_size),
        ("leading_value", format.leading_value),
        ("margin_left", format.margin_left),
        ("margin_right", format.margin_right),
    ] {
        if !value.is_finite() {
            return Err(fail(&format!("{field} is not finite")));
        }
    }
    if format.base_font_size <= 0.0 || format.entry_heading_size <= 0.0 {
        return Err(fail("font sizes must be positive"));
    }
    if format.leading_value < 0.0 {
        return Err(fail("leading must not be negative"));
    }
    if format.margin_left < 0.0 || format.margin_right < 0.0 {
        return Err(fail("margins must not be negative"));
    }
    let leading = leading_em(
        format.leading_value,
        format.leading_relative,
        format.base_font_size,
    );
    if !leading.is_finite() || leading <= 0.0 {
        return Err(fail("resolved leading must be positive"));
    }
    let mut seen = HashSet::new();
    for item in items {
        if item.id.is_empty() {
            return Err(fail("item id is empty"));
        }
        if !seen.insert(item.id.as_str()) {
            return Err(fail(&format!("duplicate item id {}", item.id)));
        }
        if !item.font_size.is_finite() || item.font_size <= 0.0 {
            return Err(fail(&format!("item {} has no positive font size", item.id)));
        }
        if !item.usable_width_pt.is_finite() || item.usable_width_pt <= 0.0 {
            return Err(fail(&format!("item {} has no positive width", item.id)));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct ResultRow {
    w: f64,
    h: f64,
    lines: u64,
}

#[derive(Deserialize)]
struct CalibrationRow {
    ratios: RatioRow,
}

#[derive(Clone, Copy, Deserialize)]
struct RatioRow {
    #[serde(rename = "cap-reg")]
    cap_reg: f64,
    #[serde(rename = "adv-reg")]
    adv_reg: f64,
    #[serde(rename = "cap-bold")]
    cap_bold: f64,
    #[serde(rename = "adv-bold")]
    adv_bold: f64,
}

fn decode_response(
    font_hash: u64,
    format: &MeasureFormat,
    misses: &[MeasureItem],
    values: &[serde_json::Value],
    results: &mut HashMap<String, MeasureResult>,
) -> Result<()> {
    let mut rows: HashMap<&str, &serde_json::Value> = HashMap::new();
    for value in values {
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Measure(format!("{PROTOCOL_VERSION} result has no id")))?;
        if id == CALIBRATION_ID {
            continue;
        }
        if rows.insert(id, value).is_some() {
            return Err(Error::Measure(format!(
                "{PROTOCOL_VERSION} duplicated fragment {id}"
            )));
        }
    }
    let wanted: HashSet<&str> = misses.iter().map(|item| item.id.as_str()).collect();
    let found: HashSet<&str> = rows.keys().copied().collect();
    if found != wanted {
        let missing: Vec<&&str> = wanted.difference(&found).collect();
        let extra: Vec<&&str> = found.difference(&wanted).collect();
        return Err(Error::Measure(format!(
            "{PROTOCOL_VERSION} returned an incomplete fragment set (missing {missing:?}, unexpected {extra:?})"
        )));
    }
    for item in misses {
        let raw = rows.get(item.id.as_str()).copied().ok_or_else(|| {
            Error::Measure(format!("{PROTOCOL_VERSION} omitted fragment {}", item.id))
        })?;
        let row: ResultRow = serde_json::from_value((*raw).clone()).map_err(|error| {
            Error::Measure(format!("fragment {} is malformed: {error}", item.id))
        })?;
        if !row.w.is_finite() || row.w < 0.0 || !row.h.is_finite() || row.h < 0.0 {
            return Err(Error::Measure(format!(
                "fragment {} has invalid dimensions",
                item.id
            )));
        }
        if row.lines < 1 {
            return Err(Error::Measure(format!(
                "fragment {} reports no lines",
                item.id
            )));
        }
        results.insert(
            item_key(font_hash, format, item),
            MeasureResult {
                id: item.id.clone(),
                width_pt: row.w,
                height_pt: row.h,
                lines: row.lines,
                char_budget: char_budget(&item.text, row.w, item.usable_width_pt),
            },
        );
    }
    Ok(())
}

fn decode_calibration(values: &[serde_json::Value]) -> Result<MeasureCalibration> {
    let mut found: Option<MeasureCalibration> = None;
    for value in values {
        if value.get("id").and_then(serde_json::Value::as_str) != Some(CALIBRATION_ID) {
            continue;
        }
        if found.is_some() {
            return Err(Error::Measure(format!(
                "{PROTOCOL_VERSION} duplicated calibration"
            )));
        }
        let row: CalibrationRow = serde_json::from_value(value.clone())
            .map_err(|error| Error::Measure(format!("calibration is malformed: {error}")))?;
        for ratio in [
            row.ratios.cap_reg,
            row.ratios.adv_reg,
            row.ratios.cap_bold,
            row.ratios.adv_bold,
        ] {
            if !ratio.is_finite() {
                return Err(Error::Measure(format!(
                    "{PROTOCOL_VERSION} calibration is not finite"
                )));
            }
        }
        found = Some(MeasureCalibration {
            cap_ratio_regular: row.ratios.cap_reg,
            advance_ratio_regular: row.ratios.adv_reg,
            cap_ratio_bold: row.ratios.cap_bold,
            advance_ratio_bold: row.ratios.adv_bold,
        });
    }
    found.ok_or_else(|| Error::Measure(format!("{PROTOCOL_VERSION} omitted calibration")))
}
