//! Size limits for document previews.

/// Default maximum file size that will be opened for preview (25 MiB).
pub const DEFAULT_MAX_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Files larger than this are not opened at all.
    pub max_bytes: u64,
    /// Maximum uncompressed size of a single zip entry (zip-bomb guard).
    pub max_entry_bytes: u64,
    pub max_paragraphs: usize,
    pub max_slides: usize,
    pub max_rows: usize,
    pub max_cols: usize,
    pub max_sheets: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_entry_bytes: 64 * 1024 * 1024,
            max_paragraphs: 5_000,
            max_slides: 500,
            max_rows: 1_000,
            max_cols: 50,
            max_sheets: 20,
        }
    }
}

/// Human readable size, e.g. `25 MiB`.
pub fn format_bytes(n: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64).replace(".0 MiB", " MiB")
    } else if n >= 1024 {
        format!("{} KiB", n / 1024)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_bytes(25 * 1024 * 1024), "25 MiB");
        assert_eq!(format_bytes(2048), "2 KiB");
        assert_eq!(format_bytes(12), "12 B");
    }
}
