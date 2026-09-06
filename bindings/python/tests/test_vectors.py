"""Protocol conformance for the Python bindings: frozen vectors, error
paths, render paths, and cache transparency."""

import json
from pathlib import Path

import pytest

import ctypst
from ctypst import (
    Engine,
    MeasureClient,
    MeasureFormat,
    MeasureItem,
    Weight,
    char_budget,
    leading_em,
)

PROTOCOL = Path(__file__).resolve().parents[3] / "protocol" / "measure-v1"


def load_vectors():
    requests = json.loads((PROTOCOL / "requests.json").read_text())
    expected = json.loads((PROTOCOL / "expected.json").read_text())
    want = {entry["name"]: entry for entry in expected["expected"]}
    return requests["requests"], want


def ulp_distance(a: float, b: float) -> int:
    import struct

    ai = struct.unpack(">q", struct.pack(">d", a))[0]
    bi = struct.unpack(">q", struct.pack(">d", b))[0]
    return abs(ai - bi)


def test_vectors_match():
    vectors, want = load_vectors()
    for entry in vectors:
        request = entry["request"]
        fmt = request["format"]
        format = MeasureFormat(
            font=fmt["font"],
            base_font_size=fmt["baseFontSize"],
            entry_heading_size=fmt["entryHeadingSize"],
            leading_value=fmt["leadingEm"],
            margin_left=fmt["marginLeft"],
            margin_right=fmt["marginRight"],
            page_size=fmt["pageSize"],
        )
        items = [
            MeasureItem(
                id=item["id"],
                text=item["text"],
                font_size=item["fontSize"],
                weight=Weight.Bold if item["weight"] == "bold" else Weight.Regular,
                usable_width_pt=item["usableWidthPt"],
            )
            for item in request["items"]
        ]
        client = MeasureClient()
        results = client.measure_all(format, items)
        expected = want[entry["name"]]["results"]
        assert [result.id for result in results] == [row["id"] for row in expected]
        for got, row in zip(results, expected):
            assert ulp_distance(got.width_pt, row["w"]) <= 1, got.id
            assert ulp_distance(got.height_pt, row["h"]) <= 1, got.id
            assert got.lines == row["lines"], got.id
        assert client.calibration() is not None
        assert client.compile_count() == 1
        # Full hits never recompile.
        client.measure_all(format, items)
        assert client.compile_count() == 1


def test_budgets_and_leading_follow_the_frozen_formulas():
    assert char_budget("", 10.0, 20.0) is None
    assert char_budget("text", 0.0, 20.0) is None
    assert char_budget("😀 done", 32.8545, 400.0) == 78
    assert leading_em(0.6, True, 10.5) == 0.6
    assert leading_em(7.0, False, 10.5) == 0.6667


def test_invalid_requests_fail_loudly():
    client = MeasureClient()
    assert client.measure_all(MeasureFormat(), []) == []
    with pytest.raises(ValueError):
        client.measure_all(MeasureFormat(), [MeasureItem(id="", text="x")])
    with pytest.raises(ValueError):
        client.measure_all(
            MeasureFormat(),
            [MeasureItem(id="d", text="x"), MeasureItem(id="d", text="y")],
        )
    with pytest.raises(ValueError):
        client.measure_all(MeasureFormat(), [MeasureItem(id="x", text="x", font_size=0.0)])


def test_render_paths():
    engine = Engine()
    document = engine.compile('#set text(font: "Archivo", size: 11pt)\nHello *world*')
    assert document.page_count() == 1
    svg = document.svg_page(0)
    assert svg.startswith("<svg") and "<path" in svg
    with pytest.raises(Exception):
        document.svg_page(9)
    pdf = document.pdf()
    assert bytes(pdf)[:5] == b"%PDF-"
    with pytest.raises(Exception):
        engine.compile("#let x = (1 +")


def test_query_decodes_to_python_objects():
    engine = Engine()
    document = engine.compile('#metadata((a: 1, b: (true, "x"))) <probe>')
    rows = document.query("probe")
    assert rows == [{"a": 1, "b": [True, "x"]}]
