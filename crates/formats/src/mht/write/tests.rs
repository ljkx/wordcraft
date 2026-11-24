use super::*;
use craft_mime::{Limits, Part, Resources};
use wordcraft_doc::{CharProps, InlineObject, Pos, StoryRef};

/// A second picture, different from [`crate::tests::png`]: a 1×1 PNG.
const DOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// [`DOT`]'s bytes.
fn dot() -> Vec<u8> {
    craft_mime::base64::STANDARD.decode(DOT).unwrap()
}

/// A document of `text`, then one paragraph per picture showing it.
fn with_pictures(text: &str, pictures: &[&[u8]]) -> Document {
    let mut d = Document::from_text(text);
    for (n, bytes) in pictures.iter().enumerate() {
        let key = d.add_media(bytes.to_vec(), "png");
        let at = d.body.len();
        d.body.push(wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("", CharProps::default())));
        let image = InlineObject::Image {
            media: key,
            w: 12.0,
            h: 6.0,
            alt: format!("picture {n}"),
            float: Default::default(),
            crop: [0.0; 4],
            ole: None,
            fx: Default::default(),
        };
        d.insert_object(&Pos::body(at, 0), image, &CharProps::default()).unwrap();
    }
    d
}

/// The message as craft-mime's reader (the one [`super::super::import`] uses) reads it: the
/// root and its leaf parts.
fn read(message: &[u8]) -> (Part, Vec<Part>) {
    let msg = craft_mime::parse(message, &Limits::DEFAULT);
    assert!(!msg.truncated);
    let leaves = msg.root.leaves().into_iter().cloned().collect();
    (msg.root, leaves)
}

/// A part's decoded body as text.
fn text(p: &Part) -> String {
    String::from_utf8(p.data().to_vec()).unwrap()
}

/// The pictures a document shows, in order, by their bytes.
fn shown(doc: &Document) -> Vec<Vec<u8>> {
    doc.body
        .iter()
        .filter_map(|b| match &**b {
            wordcraft_doc::Block::Para(p) => p.objects.iter().find_map(|o| match o {
                InlineObject::Image { media, .. } => doc.media.get(media).map(|m| m.to_vec()),
                _ => None,
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn the_page_is_the_first_part_and_each_picture_a_part_beside_it_once() {
    let png = crate::tests::png();
    let dot = dot();
    // The first picture shown twice, then another.
    let d = with_pictures("Pictures", &[&png, &dot, &png]);
    let message = export(&d);
    let (root, parts) = read(&message);
    assert_eq!(root.declared_media_type(), Some("multipart/related"));
    assert_eq!(root.content_type.get("type"), Some("text/html"));
    assert!(root.content_type.get("boundary").unwrap().starts_with(BOUNDARY_PREFIX));
    assert_eq!(parts.len(), 3, "the page and two distinct pictures");
    let page = &parts[0];
    assert_eq!(page.declared_media_type(), Some("text/html"));
    assert_eq!(page.content_type.get("charset"), Some("utf-8"));
    assert_eq!(page.content_location(), Some(PAGE_LOCATION));
    assert!(craft_mime::start_part(&root).is_none(), "no `start`: the root is the first part");
    let html = text(page);
    assert!(!html.contains("data:"), "no picture left in the page: {html}");
    // Named by Content-ID: Chrome and Edge show no `file:` part, Word links rather than keeps one
    // at the root of `file:///`.
    assert_eq!(html.matches("<img src=\"cid:image001.png\"").count(), 2, "{html}");
    assert_eq!(html.matches("<img src=\"cid:image002.png\"").count(), 1, "{html}");
    assert!(!html.contains("document_files"), "{html}");
    // The picture's other attributes stay.
    assert!(html.contains("<img src=\"cid:image002.png\" alt=\"picture 1\" width=\"16\" height=\"8\">"), "{html}");
    let resources = Resources::of(&root);
    for (part, name, bytes) in [(&parts[1], "image001.png", &png), (&parts[2], "image002.png", &dot)] {
        assert_eq!(part.declared_media_type(), Some("image/png"));
        assert_eq!(part.transfer_encoding(), "base64");
        assert_eq!(part.content_id(), Some(name), "the Content-ID the page names");
        let location = format!("file:///{FILES}/{name}");
        assert_eq!(part.content_location(), Some(location.as_str()));
        assert_eq!(part.data(), bytes.as_slice(), "{name}");
        // The reader finds it by the page's `cid:` and by its location.
        assert_eq!(resources.find(&format!("cid:{name}"), PAGE_LOCATION).map(Part::data), Some(bytes.as_slice()));
        assert_eq!(resources.find(&format!("{FILES}/{name}"), PAGE_LOCATION).map(Part::data), Some(bytes.as_slice()));
    }
    // Opened again, the pictures are where they were: the first one shown twice.
    let back = crate::mht::import(&message);
    assert_eq!(shown(&back), vec![png.clone(), dot, png]);
    assert_eq!(back.plain_text(StoryRef::Body).lines().next(), Some("Pictures"));
}

#[test]
fn every_line_is_short_ascii_and_ends_in_crlf() {
    let long = "Dobar dan, gospođo — 日本語のテキスト = \"quoted\"\t".repeat(40);
    let mut d = with_pictures(&format!("{long}\nshort line \nlast"), &[&crate::tests::png()]);
    d.core.title = "Квартальный отчёт 年度報告書 📈 ".repeat(6);
    let message = export(&d);
    assert!(message.is_ascii());
    let s = String::from_utf8(message.clone()).unwrap();
    for (i, _) in s.match_indices('\n') {
        assert_eq!(s.as_bytes().get(i.wrapping_sub(1)), Some(&b'\r'), "a bare LF at {i}");
    }
    for line in s.split("\r\n") {
        // Header lines fold within 78 characters, encoded body lines within 76.
        assert!(line.len() <= 78, "{} characters: {line}", line.len());
        assert!(!line.ends_with([' ', '\t']), "white space ends {line:?}");
    }
    // The page went quoted-printable and comes back as the HTML export wrote it, line ends as LF.
    let (_, parts) = read(&message);
    assert_eq!(parts[0].transfer_encoding(), "quoted-printable");
    let page = text(&parts[0]).replace("\r\n", "\n");
    let written = crate::html::export(&d);
    let start = written.find("<img src=\"data:").unwrap();
    assert_eq!(page.get(..start), written.get(..start), "the page up to its picture, byte for byte");
    assert_eq!(crate::mht::import(&message).plain_text(StoryRef::Body), d.plain_text(StoryRef::Body));
}

#[test]
fn text_holding_the_boundary_does_not_end_a_part() {
    // Lines that read like this writer's delimiters, and a fake picture tag, as text.
    let tricky = format!("--{BOUNDARY_PREFIX}\n--{BOUNDARY_PREFIX}--\n<img src=\"data:image/png;base64,{DOT}\">");
    let d = Document::from_text(&tricky);
    let message = String::from_utf8(export(&d)).unwrap();
    let (root, parts) = read(message.as_bytes());
    let boundary = root.content_type.get("boundary").unwrap();
    assert!(!text(&parts[0]).contains(boundary), "{message}");
    let delimiters = message.split("\r\n").filter(|l| l.starts_with(&format!("--{boundary}"))).count();
    assert_eq!(delimiters, 2, "the page's delimiter and the closing one: {message}");
    assert_eq!(parts.len(), 1, "text that reads like a picture is no picture");
    let back = crate::mht::import(message.as_bytes());
    assert_eq!(back.plain_text(StoryRef::Body), d.plain_text(StoryRef::Body));
    assert!(back.media.is_empty());
}

#[test]
fn the_title_is_the_subject_in_encoded_words() {
    let mut d = Document::from_text("x");
    d.core.title = "Izvještaj\r\nBcc: someone@example.com".into();
    let message = export(&d);
    let s = String::from_utf8(message.clone()).unwrap();
    // The message's headers end at the first empty line (the page's own `<title>` keeps its text).
    let head = s.split("\r\n\r\n").next().unwrap();
    assert!(!head.contains("\r\nBcc:"), "a line break in the title starts no header: {head}");
    let (root, _) = read(&message);
    assert!(root.headers.get("bcc").is_none());
    let subject_of = |root: &Part| root.headers.get("subject").map(craft_mime::decode_words);
    assert_eq!(subject_of(&root).as_deref(), Some("Izvještaj  Bcc: someone@example.com"));
    // A long title in several scripts comes back, cut at its first 256 characters and trimmed;
    // every encoded-word is whole characters (RFC 2047 §5), so each decodes on its own.
    let long = "Квартальный отчёт 年度報告書 📈 ".repeat(12);
    d.core.title = long.clone();
    let (root, _) = read(&export(&d));
    let value = root.headers.get("subject").unwrap();
    assert!(value.split_whitespace().all(|w| !craft_mime::decode_words(w).contains('\u{FFFD}')), "{value}");
    assert_eq!(subject_of(&root).as_deref(), Some(long.chars().take(MAX_SUBJECT_CHARS).collect::<String>().trim()));
    // A plain title is written as it is.
    d.core.title = "Quarterly report".into();
    assert!(String::from_utf8(export(&d)).unwrap().starts_with("Subject: Quarterly report\r\nMIME-Version: 1.0\r\n"));
    // No title, no subject.
    for empty in ["", " \t\r\n"] {
        assert_eq!(subject(empty), None);
    }
    assert!(String::from_utf8(export(&Document::from_text("x"))).unwrap().starts_with("MIME-Version: 1.0\r\n"));
}

#[test]
fn only_base64_data_pictures_become_parts() {
    // A page given by hand: a picture by URL, one percent-encoded, one that isn't base64, one
    // with no closing quote.
    let html = format!(
        "<p><img src=\"https://example.test/a.png\"><img src=\"data:image/png,%89PNG\"><img src=\"data:image/png;base64,abc\">\
         <img src=\"data:image/png;base64,{DOT}\" alt=\"dot\"><img src=\"data:image/png;base64,{DOT}"
    );
    let message = export_html(&html, "");
    let (_, parts) = read(&message);
    assert_eq!(parts.len(), 2, "the page and the one well-formed picture");
    assert_eq!(parts[1].data(), dot().as_slice());
    let page = text(&parts[0]);
    assert!(
        page.contains("<img src=\"https://example.test/a.png\"><img src=\"data:image/png,%89PNG\"><img src=\"data:image/png;base64,abc\">"),
        "{page}"
    );
    assert!(page.contains("<img src=\"cid:image001.png\" alt=\"dot\">"), "{page}");
    assert!(page.ends_with(&format!("<img src=\"data:image/png;base64,{DOT}")), "{page}");
    assert_eq!(extension("image/jpeg"), "jpg");
    assert_eq!(extension("application/octet-stream"), "bin");
    assert_eq!(data_uri("data:image/png;base64,ab\"c"), None);
    assert_eq!(data_uri("data:;base64,abc"), None);
    assert_eq!(data_uri("data:image/png;base64,"), None);
}

#[test]
fn a_picture_shown_ten_thousand_times_is_one_part() {
    let html = format!("<p>{}</p>", format!("<img src=\"data:image/png;base64,{DOT}\" alt=\"\">").repeat(10_000));
    let message = export_html(&html, "Many");
    let (_, parts) = read(&message);
    assert_eq!(parts.len(), 2);
    // The page keeps its 10,000 pictures, by name: about the size of their tags.
    let page_len = text(&parts[0]).len();
    assert!(page_len < 10_000 * 50, "{page_len}");
    assert_eq!(export_html(&html, "Many"), message, "the same page gives the same bytes");
}
