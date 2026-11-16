//! The frozen ctypst-measure-v1 request vectors, shipped with the crate so dependents (facades,
//! ports) replay exactly the vectors of the release they link.

use crate::measure::{MeasureFormat, MeasureItem, Weight};

/// `protocol/measure-v1/requests.json`.
pub const REQUESTS: &str = include_str!("../protocol/measure-v1/requests.json");
/// `protocol/measure-v1/expected.json`.
pub const EXPECTED: &str = include_str!("../protocol/measure-v1/expected.json");

/// One named request vector, decoded.
#[derive(Clone, Debug)]
pub struct Vector {
    /// Vector name (the key into [`EXPECTED`]).
    pub name: String,
    /// The measurement format.
    pub format: MeasureFormat,
    /// The items to measure.
    pub items: Vec<MeasureItem>,
}

/// Decode [`REQUESTS`] into typed vectors.
///
/// # Panics
/// If the shipped vector file is malformed (a packaging bug).
#[must_use]
pub fn vectors() -> Vec<Vector> {
    let requests: serde_json::Value =
        serde_json::from_str(REQUESTS).expect("requests.json is valid JSON");
    requests["requests"]
        .as_array()
        .expect("request vectors")
        .iter()
        .map(|entry| {
            let request = &entry["request"];
            let f = &request["format"];
            Vector {
                name: entry["name"].as_str().expect("named request").to_owned(),
                format: MeasureFormat {
                    font: f["font"].as_str().expect("font").to_owned(),
                    base_font_size: f["baseFontSize"].as_f64().expect("base size"),
                    entry_heading_size: f["entryHeadingSize"].as_f64().expect("heading size"),
                    leading_value: f["leadingEm"].as_f64().expect("leading"),
                    leading_relative: true,
                    margin_left: f["marginLeft"].as_f64().expect("left margin"),
                    margin_right: f["marginRight"].as_f64().expect("right margin"),
                    page_size: f["pageSize"].as_str().expect("page size").to_owned(),
                },
                items: request["items"]
                    .as_array()
                    .expect("request items")
                    .iter()
                    .map(|item| MeasureItem {
                        id: item["id"].as_str().expect("id").to_owned(),
                        text: item["text"].as_str().expect("text").to_owned(),
                        font_size: item["fontSize"].as_f64().expect("size"),
                        weight: if item["weight"] == "bold" {
                            Weight::Bold
                        } else {
                            Weight::Regular
                        },
                        usable_width_pt: item["usableWidthPt"].as_f64().expect("width"),
                    })
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn vectors_decode() {
        let vectors = super::vectors();
        assert!(!vectors.is_empty());
        assert!(vectors.iter().all(|vector| !vector.items.is_empty()));
    }
}
