"""ctypst for Python: embedded Typst measurement and rendering.

Thin over the Rust API with identical semantics. See
https://github.com/corbet-labs/ctypst for the versioned protocol,
conformance vectors, and licensing.
"""

from ._core import (
    PROTOCOL_VERSION,
    QUERY_LABEL,
    Document,
    Engine,
    MeasureCalibration,
    MeasureClient,
    MeasureFormat,
    MeasureItem,
    MeasureResult,
    Weight,
    __version__,
    char_budget,
    leading_em,
)

__all__ = [
    "PROTOCOL_VERSION",
    "QUERY_LABEL",
    "Document",
    "Engine",
    "MeasureCalibration",
    "MeasureClient",
    "MeasureFormat",
    "MeasureItem",
    "MeasureResult",
    "Weight",
    "__version__",
    "char_budget",
    "leading_em",
]
