//! Embedding fonts on save (#6329): File › Options › Save's "Embed fonts in the file"
//! (`w:embedTrueTypeFonts`, `w:embedSystemFonts`, `w:saveSubsetFonts`, ECMA-376 Part 1
//! §17.15.1.40/41/74) and the font parts a save writes (§17.8.1, §17.8.3.3–6). Synthetic font
//! bytes and hand-written fixtures; the font names are made up.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};

use wordcraft_doc::{CharProps, Document, Paragraph, para_block};
use wordcraft_docx::{EmbedStyle, FontFile};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn text(files: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(files.get(name).unwrap_or_else(|| panic!("no {name} in {:?}", files.keys())).clone()).unwrap()
}

/// Distinct bytes standing in for a font file (the writer doesn't look inside).
fn font_bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

/// A document with a paragraph in each of `fonts`, embedding on.
fn doc_in(fonts: &[&str]) -> Document {
    let mut d = Document::new();
    d.body = fonts
        .iter()
        .map(|f| para_block(Paragraph::with_text("Embedded text", CharProps { font: Some((*f).into()), ..Default::default() })))
        .collect();
    d.settings.embed_fonts = true;
    d
}

/// The font parts of a saved package.
fn font_parts(files: &BTreeMap<String, Vec<u8>>) -> Vec<&String> {
    files.keys().filter(|k| k.starts_with("word/fonts/")).collect()
}

#[test]
fn fonts_the_document_uses_are_embedded_and_read_back_byte_for_byte() {
    let regular = font_bytes(1, 3000);
    let bold = font_bytes(2, 2500);
    let asked = RefCell::new(Vec::new());
    let provider = |name: &str| {
        asked.borrow_mut().push(name.to_string());
        if name != "Quill Serif" {
            return Vec::new();
        }
        // Italic is the Regular file again (one file, two styles): written once.
        vec![
            FontFile { style: EmbedStyle::Regular, data: regular.clone() },
            FontFile { style: EmbedStyle::Bold, data: bold.clone() },
            FontFile { style: EmbedStyle::Italic, data: regular.clone() },
        ]
    };
    let doc = doc_in(&["Quill Serif", "Not Installed Sans"]);
    let saved = wordcraft_docx::write_with_fonts(&doc, wordcraft_docx::Flavor::Document, &provider).unwrap();
    assert!(asked.borrow().iter().any(|n| n == "Quill Serif") && asked.borrow().iter().any(|n| n == "Not Installed Sans"), "{:?}", asked.borrow());

    let files = unzip(&saved);
    let parts = font_parts(&files);
    assert_eq!(parts.len(), 2, "the shared file once: {parts:?}");
    // Obfuscated (§17.8.1): the first 32 bytes differ, the rest are the font's.
    for p in &parts {
        let stored = &files[*p];
        let original = if stored.len() == regular.len() { &regular } else { &bold };
        assert_ne!(stored[..32], original[..32], "{p} is obfuscated");
        assert_eq!(stored[32..], original[32..], "{p} past the first 32 bytes");
    }
    let table = text(&files, "word/fontTable.xml");
    assert_eq!(table.matches("<w:font ").count(), 1, "one entry, for the font embedded: {table}");
    for e in ["w:embedRegular", "w:embedBold", "w:embedItalic"] {
        assert!(table.contains(e), "{e}: {table}");
    }
    assert!(!table.contains("w:embedBoldItalic"), "no file, no reference: {table}");
    assert!(table.contains("w:fontKey=\"{"), "{table}");
    assert!(text(&files, "word/_rels/document.xml.rels").contains("/fontTable\""));
    let ct = text(&files, "[Content_Types].xml");
    assert!(ct.contains("fontTable+xml") && ct.matches("obfuscatedFont").count() == 2, "{ct}");
    assert!(text(&files, "word/settings.xml").contains("<w:embedTrueTypeFonts/>"));

    // Read back: the setting, and the fonts byte for byte under the name the text asks for.
    let back = wordcraft_docx::read(&saved).unwrap();
    assert!(back.settings.embed_fonts && !back.settings.embed_system_fonts && !back.settings.save_subset_fonts);
    let fonts = wordcraft_docx::embedded_fonts(&back);
    assert_eq!(fonts.len(), 2, "{:?}", fonts.iter().map(|f| (&f.name, f.data.len())).collect::<Vec<_>>());
    assert!(fonts.iter().all(|f| f.name == "Quill Serif"));
    assert_eq!(fonts[0].data, regular);
    assert_eq!(fonts[1].data, bold);

    // Each save obfuscates under a fresh key.
    let again = unzip(&wordcraft_docx::write_with_fonts(&doc, wordcraft_docx::Flavor::Document, &provider).unwrap());
    let key = |t: &str| t.split("w:fontKey=\"").nth(1).map(|k| k[..38].to_string()).unwrap();
    assert_ne!(key(&table), key(&text(&again, "word/fontTable.xml")));
}

#[test]
fn nothing_is_embedded_or_asked_for_without_the_setting() {
    let mut doc = doc_in(&["Quill Serif"]);
    doc.settings.embed_fonts = false;
    let provider = |_: &str| -> Vec<FontFile> { panic!("asked for a font with embedding off") };
    let files = unzip(&wordcraft_docx::write_with_fonts(&doc, wordcraft_docx::Flavor::Document, &provider).unwrap());
    assert!(!files.contains_key("word/fontTable.xml") && font_parts(&files).is_empty(), "{:?}", files.keys());
    assert!(!text(&files, "word/settings.xml").contains("embedTrueTypeFonts"));
    // On, with nothing installed to embed: still no table.
    doc.settings.embed_fonts = true;
    let files = unzip(&wordcraft_docx::write_with_fonts(&doc, wordcraft_docx::Flavor::Document, &|_: &str| Vec::new()).unwrap());
    assert!(!files.contains_key("word/fontTable.xml"), "{:?}", files.keys());
}

/// A package whose font table embeds "Quill Sans" (with the entry's other details) and lists
/// "Quill Mono" without embedding it; text in both, and `settings`.
fn package_with_table(settings: &str, kept_font: &[u8]) -> Vec<u8> {
    let rels = |list: &[(&str, &str, &str)]| {
        let mut s = String::from(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        for (id, ty, target) in list {
            s.push_str(&format!(r#"<Relationship Id="{id}" Type="{REL}/{ty}" Target="{target}"/>"#));
        }
        s.push_str("</Relationships>");
        s.into_bytes()
    };
    let run = |font: &str| format!(r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/></w:rPr><w:t>In {font}</w:t></w:r></w:p>"#);
    let body = format!("{}{}", run("Quill Sans"), run("Quill Mono"));
    // The kept font is stored with the all-zero key (no `w:fontKey`): as is.
    let table = format!(
        r#"<w:fonts xmlns:w="{W}" xmlns:r="{REL}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><w:font w:name="Quill Sans"><w:panose1 w:val="020B0604020202020204"/><w:charset w:val="00"/><w:family w:val="swiss"/><w:pitch w:val="variable"/><w:embedRegular r:id="rId7"/></w:font><w:font w:name="Quill Mono"><w:altName w:val="Quill Typewriter"/><w:family w:val="modern"/><w:pitch w:val="fixed"/></w:font></w:fonts>"#
    );
    zip(&[
        (
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="ttf" ContentType="application/x-font-ttf"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/fontTable.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml"/></Types>"#.to_vec(),
        ),
        ("_rels/.rels", rels(&[("rId1", "officeDocument", "word/document.xml")])),
        ("word/document.xml", format!(r#"<w:document xmlns:w="{W}"><w:body>{body}</w:body></w:document>"#).into_bytes()),
        ("word/_rels/document.xml.rels", rels(&[("rId1", "fontTable", "fontTable.xml"), ("rId2", "settings", "settings.xml")])),
        ("word/settings.xml", format!(r#"<w:settings xmlns:w="{W}">{settings}</w:settings>"#).into_bytes()),
        ("word/fontTable.xml", table.into_bytes()),
        ("word/_rels/fontTable.xml.rels", rels(&[("rId7", "font", "fonts/font1.ttf")])),
        ("word/fonts/font1.ttf", kept_font.to_vec()),
    ])
}

#[test]
fn a_kept_font_table_keeps_its_entries_and_fonts_and_gains_the_new_ones() {
    let kept = font_bytes(7, 1500);
    let mono = font_bytes(9, 1800);
    let doc = wordcraft_docx::read(&package_with_table("<w:embedTrueTypeFonts/>", &kept)).unwrap();
    assert!(doc.settings.embed_fonts);
    let asked = RefCell::new(Vec::new());
    let provider = |name: &str| {
        asked.borrow_mut().push(name.to_string());
        match name {
            "Quill Mono" => vec![FontFile { style: EmbedStyle::Regular, data: mono.clone() }],
            // Already embedded by the file: not asked for, so this would show up twice.
            "Quill Sans" => vec![FontFile { style: EmbedStyle::Regular, data: font_bytes(8, 900) }],
            _ => Vec::new(),
        }
    };
    let saved = wordcraft_docx::write_with_fonts(&doc, wordcraft_docx::Flavor::Document, &provider).unwrap();
    assert!(!asked.borrow().iter().any(|n| n == "Quill Sans"), "the file's own font isn't embedded again: {:?}", asked.borrow());
    let files = unzip(&saved);
    let table = text(&files, "word/fontTable.xml");
    // The entries keep what they said; the new font goes into its existing entry, last.
    assert_eq!(table.matches("<w:font ").count(), 2, "{table}");
    assert!(table.contains(r#"<w:altName w:val="Quill Typewriter"/><w:family w:val="modern"/><w:pitch w:val="fixed"/><w:embedRegular "#), "{table}");
    assert!(table.contains(r#"<w:panose1 w:val="020B0604020202020204"/>"#), "{table}");
    let back = wordcraft_docx::read(&saved).unwrap();
    let fonts: Vec<(String, Vec<u8>)> = wordcraft_docx::embedded_fonts(&back).into_iter().map(|f| (f.name, f.data)).collect();
    assert_eq!(fonts, vec![("Quill Sans".to_string(), kept), ("Quill Mono".to_string(), mono)]);
}

#[test]
fn the_settings_round_trip_and_a_merged_settings_part_follows_the_model() {
    // A settings part with something the model doesn't hold (rsids) is merged into on save.
    let settings =
        r#"<w:embedTrueTypeFonts/><w:embedSystemFonts w:val="1"/><w:saveSubsetFonts w:val="true"/><w:rsids><w:rsidRoot w:val="00AB12CD"/></w:rsids>"#;
    let mut doc = wordcraft_docx::read(&package_with_table(settings, &font_bytes(7, 100))).unwrap();
    let s = &doc.settings;
    assert!(s.embed_fonts && s.embed_system_fonts && s.save_subset_fonts);
    let out = text(&unzip(&wordcraft_docx::write(&doc).unwrap()), "word/settings.xml");
    let at = |n: &str| out.find(n).unwrap_or_else(|| panic!("{n} missing from {out}"));
    assert!(at("<w:embedTrueTypeFonts/>") < at("<w:embedSystemFonts/>") && at("<w:embedSystemFonts/>") < at("<w:saveSubsetFonts/>"), "{out}");
    assert!(out.contains("00AB12CD"), "the rest is kept: {out}");
    // Turned off in the model: gone from the merged part too.
    doc.settings.embed_fonts = false;
    doc.settings.embed_system_fonts = false;
    let out = text(&unzip(&wordcraft_docx::write(&doc).unwrap()), "word/settings.xml");
    assert!(!out.contains("embedTrueTypeFonts") && !out.contains("embedSystemFonts") && out.contains("<w:saveSubsetFonts/>"), "{out}");
    // `w:val="0"` reads as off.
    let off = wordcraft_docx::read(&package_with_table(r#"<w:embedTrueTypeFonts w:val="0"/>"#, &font_bytes(7, 100))).unwrap();
    assert!(!off.settings.embed_fonts);
}

#[test]
fn fonts_past_the_reference_limit_are_left_out_whole() {
    // Twenty fonts of four files each are 80 references; the reader takes 64, so 16 fonts go in
    // whole and the rest stay out (never a font with only some of its styles).
    let names: Vec<String> = (0..20).map(|i| format!("Quill Face {i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let provider = |name: &str| {
        let n: u8 = name.rsplit(' ').next().and_then(|n| n.parse().ok()).unwrap_or(99);
        EmbedStyle::ALL
            .iter()
            .enumerate()
            .map(|(i, s)| FontFile { style: *s, data: font_bytes(n.wrapping_mul(4).wrapping_add(i as u8), 64) })
            .collect()
    };
    let saved = wordcraft_docx::write_with_fonts(&doc_in(&refs), wordcraft_docx::Flavor::Document, &provider).unwrap();
    let back = wordcraft_docx::read(&saved).unwrap();
    let fonts = wordcraft_docx::embedded_fonts(&back);
    assert_eq!(fonts.len(), 64, "every reference written reads back");
    let mut per_font: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &fonts {
        *per_font.entry(f.name.as_str()).or_default() += 1;
    }
    assert_eq!(per_font.len(), 16);
    assert!(per_font.values().all(|n| *n == 4), "{per_font:?}");
}

#[test]
fn hostile_providers_and_names_stay_bounded() {
    // A provider repeating a style, giving empty files or answering every name: one file per
    // style, empty ones dropped, and a document naming thousands of fonts asks about a bounded
    // number.
    let names: Vec<String> = (0..2000).map(|i| format!("Font\u{0}{i}<&\"")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let asked = RefCell::new(0usize);
    let provider = |_: &str| {
        *asked.borrow_mut() += 1;
        vec![
            FontFile { style: EmbedStyle::Regular, data: font_bytes(1, 40) },
            FontFile { style: EmbedStyle::Regular, data: font_bytes(2, 40) },
            FontFile { style: EmbedStyle::Bold, data: Vec::new() },
        ]
    };
    let saved = wordcraft_docx::write_with_fonts(&doc_in(&refs), wordcraft_docx::Flavor::Document, &provider).unwrap();
    assert!(*asked.borrow() <= 256, "asked {} times", asked.borrow());
    let back = wordcraft_docx::read(&saved).unwrap();
    let fonts = wordcraft_docx::embedded_fonts(&back);
    assert_eq!(fonts.len(), 64);
    assert!(fonts.iter().all(|f| f.data == font_bytes(1, 40)), "the first file for the style");
    let files = unzip(&saved);
    assert_eq!(font_parts(&files).len(), 1, "one distinct file, written once");
}
