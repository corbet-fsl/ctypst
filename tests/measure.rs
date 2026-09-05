//! Conformance of the native measurement adapter against the frozen
//! protocol vectors, plus the fail-loud validation and batching contracts.
//!
//! Needs the bundled fonts, so this target is empty without the
//! `document-fonts` feature.

#![cfg(feature = "document-fonts")]

use std::path::PathBuf;

use ctypst::measure::{MeasureClient, MeasureFormat, MeasureItem, Weight, char_budget, leading_em};

fn protocol_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocol/measure-v1")
}

fn load_vectors() -> (serde_json::Value, serde_json::Value) {
    let dir = protocol_dir();
    let requests: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("requests.json")).expect("requests.json is readable"),
    )
    .expect("requests.json is valid");
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("expected.json")).expect("expected.json is readable"),
    )
    .expect("expected.json is valid");
    (requests, expected)
}

fn weight(name: &str) -> Weight {
    match name {
        "bold" => Weight::Bold,
        _ => Weight::Regular,
    }
}

fn client() -> MeasureClient {
    MeasureClient::new(ctypst::fonts::documents()).expect("engine builds on bundled fonts")
}

/// Assert two floats are equal within one unit in the last place.
///
/// A few measured and derived values sit exactly on IEEE rounding
/// boundaries and wobble by one ulp across hosts and builds, while ids,
/// counts, and line numbers stay bitwise exact. One ulp (2e-16 relative)
/// is fourteen orders of magnitude below any product threshold, so the
/// vectors pin it instead of pretending floats are canonical.
fn assert_ulp_eq(left: f64, right: f64, what: &str) {
    let distance = left.to_bits().abs_diff(right.to_bits());
    assert!(
        distance <= 1,
        "{what} differs by {distance} ulp: {left} vs {right}"
    );
}

fn fixture_format() -> MeasureFormat {
    MeasureFormat {
        font: "Archivo".to_owned(),
        base_font_size: 9.5,
        entry_heading_size: 11.0,
        leading_value: 0.66,
        leading_relative: true,
        margin_left: 19.0,
        margin_right: 19.0,
        page_size: "a4".to_owned(),
    }
}

fn fixture_item(id: &str) -> MeasureItem {
    MeasureItem {
        id: id.to_owned(),
        text: "Evidence.".to_owned(),
        font_size: 10.5,
        weight: Weight::Regular,
        usable_width_pt: 400.0,
    }
}

#[test]
fn vectors_match_exactly_on_native() {
    let (requests, expected) = load_vectors();
    let expected_by_name: std::collections::HashMap<&str, &serde_json::Value> =
        expected["expected"]
            .as_array()
            .expect("expected vectors")
            .iter()
            .map(|entry| (entry["name"].as_str().expect("named vector"), entry))
            .collect();
    for entry in requests["requests"].as_array().expect("request vectors") {
        let name = entry["name"].as_str().expect("named request");
        let request = &entry["request"];
        let format_value = &request["format"];
        let format = MeasureFormat {
            font: format_value["font"].as_str().expect("font").to_owned(),
            base_font_size: format_value["baseFontSize"].as_f64().expect("base size"),
            entry_heading_size: format_value["entryHeadingSize"]
                .as_f64()
                .expect("heading size"),
            leading_value: format_value["leadingEm"].as_f64().expect("leading"),
            leading_relative: true,
            margin_left: format_value["marginLeft"].as_f64().expect("left margin"),
            margin_right: format_value["marginRight"].as_f64().expect("right margin"),
            page_size: format_value["pageSize"]
                .as_str()
                .expect("page size")
                .to_owned(),
        };
        let items: Vec<MeasureItem> = request["items"]
            .as_array()
            .expect("request items")
            .iter()
            .map(|item| MeasureItem {
                id: item["id"].as_str().expect("id").to_owned(),
                text: item["text"].as_str().expect("text").to_owned(),
                font_size: item["fontSize"].as_f64().expect("size"),
                weight: weight(item["weight"].as_str().expect("weight")),
                usable_width_pt: item["usableWidthPt"].as_f64().expect("width"),
            })
            .collect();
        let mut client = client();
        let results = client
            .measure_all(&format, &items)
            .expect("vector measures");
        let want = expected_by_name[name];
        let want_results = want["results"].as_array().expect("want results");
        assert_eq!(results.len(), want_results.len(), "result count for {name}");
        for (got, want) in results.iter().zip(want_results) {
            assert_eq!(
                got.id,
                want["id"].as_str().expect("want id"),
                "id order for {name}"
            );
            assert_ulp_eq(got.width_pt, want["w"].as_f64().expect("want w"), "width");
            assert_ulp_eq(got.height_pt, want["h"].as_f64().expect("want h"), "height");
            assert_eq!(
                got.lines,
                want["lines"].as_u64().expect("want lines"),
                "lines for {}",
                got.id
            );
            let text = items
                .iter()
                .find(|item| item.id == got.id)
                .expect("item text");
            assert_eq!(
                got.char_budget,
                char_budget(&text.text, got.width_pt, text.usable_width_pt),
                "budget for {}",
                got.id
            );
        }
        let calibration = client.calibration().expect("calibration is reported");
        let ratios = &want["calibration"]["ratios"];
        assert_ulp_eq(
            calibration.cap_ratio_regular,
            ratios["cap-reg"].as_f64().expect("cap-reg"),
            "cap-reg",
        );
        assert_ulp_eq(
            calibration.advance_ratio_regular,
            ratios["adv-reg"].as_f64().expect("adv-reg"),
            "adv-reg",
        );
        assert_ulp_eq(
            calibration.cap_ratio_bold,
            ratios["cap-bold"].as_f64().expect("cap-bold"),
            "cap-bold",
        );
        assert_ulp_eq(
            calibration.advance_ratio_bold,
            ratios["adv-bold"].as_f64().expect("adv-bold"),
            "adv-bold",
        );
    }
}

#[test]
fn budgets_follow_the_frozen_formula() {
    assert_eq!(char_budget("", 10.0, 20.0), None);
    assert_eq!(char_budget("text", 0.0, 20.0), None);
    assert_eq!(char_budget("😀 done", 32.8545, 400.0), Some(78));
    assert_eq!(char_budget("Evidence.", 100.0, 200.0), Some(9));
}

#[test]
fn leading_normalization_matches_the_legacy_rule() {
    assert_ulp_eq(leading_em(0.6, true, 10.5), 0.6, "relative leading");
    assert_ulp_eq(leading_em(0.66, true, 9.5), 0.66, "relative leading");
    assert_ulp_eq(leading_em(7.0, false, 10.5), 0.6667, "absolute leading");
    assert_ulp_eq(leading_em(6.93, false, 10.5), 0.66, "absolute leading");
}

#[test]
fn invalid_requests_fail_loudly() {
    let format = fixture_format();
    let mut bad_item = fixture_item("x");
    let mut client = client();
    assert!(
        client
            .measure_all(&format, &[])
            .expect("empty is fine")
            .is_empty()
    );
    assert_eq!(client.compile_count(), 0, "empty request never compiles");

    bad_item.id.clear();
    assert!(client.measure_all(&format, &[bad_item.clone()]).is_err());

    let duplicates = vec![fixture_item("dup"), fixture_item("dup")];
    assert!(client.measure_all(&format, &duplicates).is_err());

    bad_item = fixture_item("x");
    bad_item.font_size = 0.0;
    assert!(client.measure_all(&format, &[bad_item.clone()]).is_err());
    bad_item.font_size = f64::NAN;
    assert!(client.measure_all(&format, &[bad_item.clone()]).is_err());
    bad_item.font_size = 10.5;
    bad_item.usable_width_pt = -1.0;
    assert!(client.measure_all(&format, &[bad_item]).is_err());

    let mut bad_format = format.clone();
    bad_format.base_font_size = 0.0;
    assert!(
        client
            .measure_all(&bad_format, &[fixture_item("x")])
            .is_err()
    );
    bad_format = format.clone();
    bad_format.margin_left = -1.0;
    assert!(
        client
            .measure_all(&bad_format, &[fixture_item("x")])
            .is_err()
    );
}

#[test]
fn cache_batches_misses_and_never_recompiles_hits() {
    let format = fixture_format();
    let mut client = client();
    let first = vec![fixture_item("a"), fixture_item("b")];
    let results = client.measure_all(&format, &first).expect("cold measures");
    assert_eq!(
        results
            .iter()
            .map(|result| result.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(client.compile_count(), 1);

    let reordered = vec![fixture_item("b"), fixture_item("a")];
    let results = client
        .measure_all(&format, &reordered)
        .expect("hits assemble");
    assert_eq!(
        results
            .iter()
            .map(|result| result.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(client.compile_count(), 1, "full hits never recompile");

    let grown = vec![fixture_item("a"), fixture_item("b"), fixture_item("c")];
    client
        .measure_all(&format, &grown)
        .expect("one miss compiles");
    assert_eq!(client.compile_count(), 2, "one compile per miss batch");

    let mut changed = format.clone();
    changed.margin_left = 12.0;
    client
        .measure_all(&changed, &first)
        .expect("format change recompiles");
    assert_eq!(client.compile_count(), 3, "format change purges the cache");
}
