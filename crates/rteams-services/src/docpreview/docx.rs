//! docx / pptx text extraction (zip + quick-xml).

use std::io::Cursor;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{read_entry, DocPreview, Limits};

/// Paragraph texts from a WordprocessingML / DrawingML XML body.
/// `para` is the local name of the paragraph element (`p` for both).
fn paragraphs_from_xml(xml: &[u8], max: usize) -> (Vec<String>, bool) {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_t = false;
    let mut truncated = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"t" => in_t = true,
                b"tab" => cur.push('\t'),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"tab" => cur.push('\t'),
                b"br" | b"cr" => cur.push('\n'),
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"t" => in_t = false,
                b"p" => {
                    if out.len() >= max {
                        truncated = true;
                        break;
                    }
                    out.push(std::mem::take(&mut cur));
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_t => {
                if let Ok(s) = t.unescape() {
                    cur.push_str(&s);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if !cur.is_empty() && out.len() < max {
        out.push(cur);
    }
    (out, truncated)
}

pub fn extract_docx(bytes: &[u8], limits: &Limits) -> Result<DocPreview, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let xml = read_entry(&mut zip, "word/document.xml", limits.max_entry_bytes)?;
    let (paragraphs, truncated) = paragraphs_from_xml(&xml, limits.max_paragraphs);
    Ok(DocPreview::Document {
        paragraphs,
        truncated,
    })
}

pub fn extract_pptx(bytes: &[u8], limits: &Limits) -> Result<DocPreview, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut names: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| {
            let num = n
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse::<u32>()
                .ok()?;
            Some((num, n.to_string()))
        })
        .collect();
    names.sort();
    let truncated = names.len() > limits.max_slides;
    names.truncate(limits.max_slides);
    let mut slides = Vec::new();
    for (_, name) in names {
        let xml = read_entry(&mut zip, &name, limits.max_entry_bytes)?;
        let (paras, _) = paragraphs_from_xml(&xml, limits.max_paragraphs);
        slides.push(paras.into_iter().filter(|p| !p.trim().is_empty()).collect());
    }
    Ok(DocPreview::Slides { slides, truncated })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    pub fn build_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const DOC: &str = r#"<w:document xmlns:w="x"><w:body>
<w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> &amp; world</w:t></w:r></w:p>
<w:p><w:r><w:t>Second</w:t></w:r><w:r><w:tab/><w:t>tabbed</w:t></w:r></w:p>
</w:body></w:document>"#;

    #[test]
    fn docx_paragraphs() {
        let z = build_zip(&[("word/document.xml", DOC)]);
        let p = extract_docx(&z, &Limits::default()).unwrap();
        assert_eq!(
            p,
            DocPreview::Document {
                paragraphs: vec!["Hello & world".into(), "Second\ttabbed".into()],
                truncated: false
            }
        );
    }

    #[test]
    fn docx_truncates_and_rejects_garbage() {
        let z = build_zip(&[("word/document.xml", DOC)]);
        let l = Limits {
            max_paragraphs: 1,
            ..Limits::default()
        };
        match extract_docx(&z, &l).unwrap() {
            DocPreview::Document {
                paragraphs,
                truncated,
            } => {
                assert_eq!(paragraphs.len(), 1);
                assert!(truncated);
            }
            _ => panic!(),
        }
        assert!(extract_docx(b"not a zip", &Limits::default()).is_err());
    }

    #[test]
    fn pptx_slides_sorted() {
        let s =
            |t: &str| format!(r#"<p:sld xmlns:a="x"><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"#);
        let (s1, s2, s10) = (s("one"), s("two"), s("ten"));
        let z = build_zip(&[
            ("ppt/slides/slide10.xml", &s10),
            ("ppt/slides/slide2.xml", &s2),
            ("ppt/slides/slide1.xml", &s1),
        ]);
        match extract_pptx(&z, &Limits::default()).unwrap() {
            DocPreview::Slides { slides, .. } => {
                assert_eq!(slides, vec![vec!["one"], vec!["two"], vec!["ten"]]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn size_limit_and_unsupported() {
        let l = Limits {
            max_bytes: 4,
            ..Limits::default()
        };
        assert!(matches!(
            crate::docpreview::preview_bytes("docx", b"0123456789", &l).unwrap(),
            DocPreview::TooLarge { size: 10, limit: 4 }
        ));
        assert!(matches!(
            crate::docpreview::preview_bytes("pdf", b"x", &Limits::default()).unwrap(),
            DocPreview::Unsupported { .. }
        ));
    }
}
