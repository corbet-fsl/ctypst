//! Embedded Archivo faces and their license metadata.

/// Name of the bundled font license.
pub const LICENSE: &str = "OFL-1.1";

/// Human-readable copyright and reserved-name notices.
pub const NOTICE: &str = include_str!("../fonts/NOTICE.md");

const DOCUMENTS: &[&[u8]] = &[
    include_bytes!("../fonts/Archivo-Bold.ttf"),
    include_bytes!("../fonts/Archivo-Italic.ttf"),
    include_bytes!("../fonts/Archivo-Medium.ttf"),
    include_bytes!("../fonts/Archivo-Regular.ttf"),
];

/// Return the four bundled Archivo faces.
#[must_use]
pub const fn documents() -> &'static [&'static [u8]] {
    DOCUMENTS
}
