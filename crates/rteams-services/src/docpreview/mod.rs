//! Read-only previews of office documents: docx / pptx text, xlsx / csv tables.

use std::io::Read;
use std::path::Path;

pub mod docx;
pub mod limits;
pub mod xlsx;

pub use limits::{format_bytes, Limits, DEFAULT_MAX_BYTES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocPreview {
    /// docx: one string per paragraph.
    Document {
        paragraphs: Vec<String>,
        truncated: bool,
    },
    /// pptx: text lines per slide.
    Slides {
        slides: Vec<Vec<String>>,
        truncated: bool,
    },
    /// xlsx / csv.
    Sheets(Vec<Sheet>),
    /// File exceeds `Limits::max_bytes`.
    TooLarge { size: u64, limit: u64 },
    /// Extension has no preview (pdf and others: open externally).
    Unsupported { ext: String },
}

/// Extensions this module can preview.
pub fn is_previewable(path: &Path) -> bool {
    matches!(
        ext_of(path).as_str(),
        "docx" | "pptx" | "xlsx" | "xlsm" | "xls" | "csv"
    )
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Preview the file at `path`. Errors are human readable messages.
pub fn preview_file(path: &Path, limits: &Limits) -> Result<DocPreview, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > limits.max_bytes {
        return Ok(DocPreview::TooLarge {
            size,
            limit: limits.max_bytes,
        });
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    preview_bytes(&ext_of(path), &bytes, limits)
}

/// Preview in-memory bytes for the given (lowercase) extension.
pub fn preview_bytes(ext: &str, bytes: &[u8], limits: &Limits) -> Result<DocPreview, String> {
    if bytes.len() as u64 > limits.max_bytes {
        return Ok(DocPreview::TooLarge {
            size: bytes.len() as u64,
            limit: limits.max_bytes,
        });
    }
    match ext {
        "docx" => docx::extract_docx(bytes, limits),
        "pptx" => docx::extract_pptx(bytes, limits),
        "xlsx" | "xlsm" | "xls" => xlsx::extract_workbook(bytes, limits),
        "csv" => Ok(xlsx::extract_csv(&String::from_utf8_lossy(bytes), limits)),
        other => Ok(DocPreview::Unsupported { ext: other.into() }),
    }
}

/// Read one zip entry with an uncompressed-size cap.
pub(crate) fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> Result<Vec<u8>, String> {
    let f = zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    let mut out = Vec::new();
    f.take(max + 1)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > max {
        return Err(format!("{name}: entry too large"));
    }
    Ok(out)
}
