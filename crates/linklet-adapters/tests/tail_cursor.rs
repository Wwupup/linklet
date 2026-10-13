//! Real files are resumed without losing text across pages, appends, or resets.
use linklet_core::{result::ErrorCode, transfer::Destination, wire::TailRequest};
use std::{
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};
struct Fixture {
    dir: PathBuf,
    paths: Destination,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "linklet-tail-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("directory");
        let paths = Destination::new(&dir.to_string_lossy()).expect("base");
        Self { dir, paths }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn request() -> TailRequest {
    TailRequest {
        path: "log".into(),
        count: 2,
        cursor: None,
        content_bytes: 17,
    }
}
#[test]
fn pages_append_in_order_without_duplicate_or_missing_characters() {
    let f = Fixture::new();
    let expected = format!("{}\r\nlast unfinished", "\u{4e2d}\u{1f600}".repeat(100));
    std::fs::write(f.dir.join("log"), format!("older\n{expected}")).expect("file");
    let mut request = request();
    let mut text = String::new();
    let mut previous = None;
    for _ in 0..1000 {
        let page = linklet_adapters::tail::read(&f.paths, &request).expect("page");
        assert!(page.text.len() <= 17);
        assert!(!page.lossy);
        assert!(page.bytes_read <= 16 * 1024 * 1024);
        if let Some(end) = previous {
            assert_eq!(page.range.start, end);
        }
        previous = Some(page.range.end);
        text.push_str(&page.text);
        request.cursor = Some(page.next_cursor);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(text, expected);
    assert!(
        linklet_adapters::tail::read(&f.paths, &request)
            .expect("no new content")
            .text
            .is_empty()
    );
}
#[test]
fn half_characters_wait_for_the_next_append() {
    let f = Fixture::new();
    let path = f.dir.join("log");
    std::fs::write(&path, b"start\n").expect("file");
    let mut request = request();
    let first = linklet_adapters::tail::read(&f.paths, &request).expect("first");
    request.cursor = Some(first.next_cursor);
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append");
    append.write_all(&[0xe4, 0xb8]).expect("half");
    let half = linklet_adapters::tail::read(&f.paths, &request).expect("half page");
    assert!(half.text.is_empty());
    assert_eq!(half.range.end, 6);
    assert_eq!(half.pending_bytes, 2);
    assert!(!half.has_more);
    request.cursor = Some(half.next_cursor);
    append.write_all(&[0xad, b'\r', b'\n']).expect("rest");
    let page = linklet_adapters::tail::read(&f.paths, &request).expect("complete");
    assert_eq!(page.text, "\u{4e2d}\r\n");
    assert!(!page.lossy);
}
#[test]
fn rotation_shrink_and_boundary_rewrite_are_explicit_resets() {
    let f = Fixture::new();
    let path = f.dir.join("log");
    std::fs::write(&path, b"old content\n").expect("file");
    let mut request = request();
    let first = linklet_adapters::tail::read(&f.paths, &request).expect("first");
    request.cursor = Some(first.next_cursor);
    std::fs::rename(&path, f.dir.join("old")).expect("rotate");
    std::fs::write(&path, b"replacement\n").expect("replacement");
    let page = linklet_adapters::tail::read(&f.paths, &request).expect("replaced");
    assert_eq!(page.file_changed.as_deref(), Some("replaced"));
    assert_eq!(page.range.start, 0);
    assert_eq!(page.text, "replacement\n");
    request.cursor = Some(page.next_cursor);
    std::fs::write(&path, b"short\n").expect("shrink");
    let page = linklet_adapters::tail::read(&f.paths, &request).expect("shortened");
    assert_eq!(page.file_changed.as_deref(), Some("shortened"));
    request.cursor = Some(page.next_cursor);
    std::fs::write(&path, b"other\n").expect("rewrite");
    let page = linklet_adapters::tail::read(&f.paths, &request).expect("changed");
    assert_eq!(page.file_changed.as_deref(), Some("boundary_changed"));
    assert_eq!(page.text, "other\n");
    request.path = "old".into();
    let error = linklet_adapters::tail::read(&f.paths, &request).expect_err("path binding");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    request.path = "missing".into();
    request.cursor = None;
    assert_eq!(
        linklet_adapters::tail::read(&f.paths, &request)
            .expect_err("missing")
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn an_empty_log_can_acquire_a_legacy_encoding_without_silent_mojibake() {
    let f = Fixture::new();
    let path = f.dir.join("log");
    std::fs::write(&path, []).expect("empty");
    let mut request = request();
    request.cursor = Some(
        linklet_adapters::tail::read(&f.paths, &request)
            .expect("empty page")
            .next_cursor,
    );
    std::fs::write(&path, [0xd6, 0xd0, 0xce, 0xc4, 10]).expect("legacy append");
    let page = linklet_adapters::tail::read(&f.paths, &request).expect("legacy page");
    assert_eq!(page.file_changed.as_deref(), Some("encoding_changed"));
    assert!(!page.lossy);
    if cfg!(windows) {
        assert!(page.encoding.starts_with("cp"));
        if page.encoding == "cp936" {
            assert_eq!(page.text, "\u{4e2d}\u{6587}\n");
        }
    } else {
        assert_eq!(page.encoding, "latin-1");
    }
}
