//! Page boundaries never consume undisplayed text or partial characters.
use linklet_core::{search::Encoding, tail::decode_page};
#[test]
fn unicode_and_code_page_pagination_is_lossless() {
    for (bytes, encoding, expected) in [
        (
            "a\r\n\u{4e2d}\u{1f600}unfinished".as_bytes().to_vec(),
            Encoding::Utf8,
            "a\r\n\u{4e2d}\u{1f600}unfinished",
        ),
        (
            [
                vec![0xff, 0xfe],
                "a\r\n\u{4e2d}\u{1f600}unfinished"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ]
            .concat(),
            Encoding::Utf16Le,
            "a\r\n\u{4e2d}\u{1f600}unfinished",
        ),
        (
            vec![b'a', 0xd6, 0xd0, 0xce, 0xc4, b'\n'],
            Encoding::CodePage(936),
            "a\u{4e2d}\u{6587}\n",
        ),
    ] {
        let mut offset = 0;
        let mut text = String::new();
        while offset < bytes.len() {
            let page = decode_page(&bytes[offset..], encoding, offset as u64, 4).expect("decode");
            assert!(page.consumed > 0, "{page:?}");
            assert!(page.text.len() <= 4, "{page:?}");
            assert!(!page.lossy, "{page:?}");
            offset += page.consumed;
            text.push_str(&page.text);
        }
        assert_eq!(text, expected);
    }
}
#[test]
fn an_incomplete_character_is_not_consumed() {
    for (bytes, encoding, consumed) in [
        (vec![b'x', 0xe4, 0xb8], Encoding::Utf8, 1),
        (vec![b'x', 0, 0x3d, 0xd8], Encoding::Utf16Le, 2),
        (vec![b'x', 0xd6], Encoding::CodePage(936), 1),
    ] {
        let page = decode_page(&bytes, encoding, 0, 32).expect("decode");
        assert_eq!(page.text, "x");
        assert_eq!(page.consumed, consumed);
        assert_eq!(page.pending, bytes.len() - consumed);
        assert!(!page.lossy);
    }
}
