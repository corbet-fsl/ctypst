use typst::foundations::{Label, Selector};
use typst::introspection::Introspector;
use typst::utils::PicoStr;

use crate::{Document, Error, Result};

/// Query every metadata value carrying `label` and serialize it as JSON.
pub fn query_json(document: &Document, label: &str) -> Result<Vec<serde_json::Value>> {
    if label.trim().is_empty() {
        return Err(Error::EmptyLabel);
    }
    let label = Label::new(PicoStr::intern(label)).ok_or(Error::EmptyLabel)?;
    document
        .introspector()
        .query(&Selector::Label(label))
        .into_iter()
        .map(|content| {
            let value = content
                .field_by_name("value")
                .map_err(|error| Error::Query(error.to_string()))?;
            serde_json::to_value(value).map_err(|error| Error::Query(error.to_string()))
        })
        .collect()
}
