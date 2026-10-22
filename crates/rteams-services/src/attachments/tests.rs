use super::*;

fn png_bytes() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(40, 20, image::Rgba([1, 2, 3, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn store_copies_file() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("note.txt");
    std::fs::write(&src, "hi").unwrap();
    let a = store(t.path(), "team", &src).unwrap();
    assert_eq!(a.name, "note.txt");
    assert!(Path::new(&a.path).is_absolute());
    assert_eq!(
        std::fs::read_to_string(resolve(t.path(), "team", &a)).unwrap(),
        "hi"
    );
    assert!(a.path.contains("attachments"));
    assert_eq!(attachment_markdown(&a), format!("[note.txt]({})", a.path));
}

#[test]
fn rejects_dir_and_large_and_missing() {
    let t = tempfile::tempdir().unwrap();
    assert!(matches!(
        store(t.path(), "x", t.path()),
        Err(AttachError::IsDirectory(_))
    ));
    assert!(store(t.path(), "x", &t.path().join("nope")).is_err());
    let big = vec![0u8; MAX_BYTES as usize + 1];
    assert!(matches!(
        store_bytes(t.path(), "x", "b.bin", &big),
        Err(AttachError::TooLarge { .. })
    ));
}

#[test]
fn bytes_sanitize_and_thumbnail() {
    let t = tempfile::tempdir().unwrap();
    let a = store_bytes(t.path(), "x", "../../evil.png", &png_bytes()).unwrap();
    assert_eq!(a.name, "evil.png");
    assert!(Path::new(&a.path).starts_with(t.path().canonicalize().unwrap()));
    assert!(is_image(&a));
    assert!(attachment_markdown(&a).starts_with("![evil.png]("));
    let (w, h, px) = thumbnail(Path::new(&a.path), 10).unwrap();
    assert_eq!((w, h), (10, 5));
    assert_eq!(px.len(), (w * h * 4) as usize);
    assert!(thumbnail(&t.path().join("none.png"), 10).is_none());
    assert!(matches!(
        store_bytes(t.path(), "x", "..", b"a"),
        Err(AttachError::Invalid)
    ));
}

#[test]
fn resolve_relative() {
    let a = Attachment {
        name: "a".into(),
        path: "attachments/1/a".into(),
    };
    assert_eq!(
        resolve(Path::new("/h"), "t", &a),
        PathBuf::from("/h/t/attachments/1/a")
    );
}
