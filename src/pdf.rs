use typst::foundations::Datetime;

use crate::{Document, Engine, Error, Result};

impl Engine {
    /// Export a deterministic PDF with an explicit Unix timestamp.
    pub fn pdf(&self, document: &Document, source_date_epoch: i64) -> Result<Vec<u8>> {
        let instant = time::OffsetDateTime::from_unix_timestamp(source_date_epoch)
            .map_err(|_| Error::Timestamp(source_date_epoch))?;
        let datetime = Datetime::from_ymd_hms(
            instant.year(),
            u8::from(instant.month()),
            instant.day(),
            instant.hour(),
            instant.minute(),
            instant.second(),
        )
        .ok_or(Error::Timestamp(source_date_epoch))?;
        let options = typst_pdf::PdfOptions {
            timestamp: Some(typst_pdf::Timestamp::new_utc(datetime)),
            ..typst_pdf::PdfOptions::default()
        };
        let pdf = typst_pdf::pdf(document, &options)
            .map_err(|errors| Error::Pdf(format!("{errors:?}")))?;
        self.check_pdf_size(pdf.len())?;
        if !pdf.starts_with(b"%PDF-") {
            return Err(Error::Pdf("export did not return a PDF".to_owned()));
        }
        Ok(pdf)
    }
}
