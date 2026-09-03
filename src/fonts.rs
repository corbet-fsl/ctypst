//! Embedded document font pack and its license metadata.

/// Name of the bundled font license.
pub const LICENSE: &str = "OFL-1.1";

/// Human-readable copyright and reserved-name notices.
pub const NOTICE: &str = include_str!("../fonts/NOTICE.md");

const ARCHIVO: &[&[u8]] = &[
    include_bytes!("../fonts/Archivo-Bold.ttf"),
    include_bytes!("../fonts/Archivo-Italic.ttf"),
    include_bytes!("../fonts/Archivo-Medium.ttf"),
    include_bytes!("../fonts/Archivo-Regular.ttf"),
];

const DOCUMENTS: &[&[u8]] = &[
    ARCHIVO[0],
    ARCHIVO[1],
    ARCHIVO[2],
    ARCHIVO[3],
    include_bytes!("../fonts/eb-garamond-latin-400-italic.ttf"),
    include_bytes!("../fonts/eb-garamond-latin-400-normal.ttf"),
    include_bytes!("../fonts/eb-garamond-latin-500-normal.ttf"),
    include_bytes!("../fonts/eb-garamond-latin-700-normal.ttf"),
    include_bytes!("../fonts/ibm-plex-serif-latin-400-italic.ttf"),
    include_bytes!("../fonts/ibm-plex-serif-latin-400-normal.ttf"),
    include_bytes!("../fonts/ibm-plex-serif-latin-500-normal.ttf"),
    include_bytes!("../fonts/ibm-plex-serif-latin-700-normal.ttf"),
    include_bytes!("../fonts/source-serif-4-latin-400-italic.ttf"),
    include_bytes!("../fonts/source-serif-4-latin-400-normal.ttf"),
    include_bytes!("../fonts/source-serif-4-latin-500-normal.ttf"),
    include_bytes!("../fonts/source-serif-4-latin-700-normal.ttf"),
];

/// Return only the bundled Archivo family.
#[must_use]
pub const fn archivo() -> &'static [&'static [u8]] {
    ARCHIVO
}

/// Return the complete bundled document font pack.
#[must_use]
pub const fn documents() -> &'static [&'static [u8]] {
    DOCUMENTS
}
