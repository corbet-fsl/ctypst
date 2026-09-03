use typstyle_core::{Config, Typstyle};

use crate::{Error, Result};

/// Format Typst source deterministically with Typstyle.
pub fn format_source(source: &str, width: usize) -> Result<String> {
    let width = width.clamp(40, 240);
    let rendered = Typstyle::new(Config::new().with_width(width))
        .format_text(source.to_owned())
        .render()
        .map_err(|error| Error::Format(error.to_string()))?;
    Ok(if rendered.ends_with('\n') {
        rendered
    } else {
        format!("{rendered}\n")
    })
}
