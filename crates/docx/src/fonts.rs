//! Fonts a file embeds through its font table (#1171).
//!
//! The reader keeps the font table and its font parts as opaque passthrough parts, for writing
//! back. [`embedded_fonts`] hands their font files to whoever lays the document out, undoing the
//! obfuscation Word applies to them (ECMA-376 Part 1, 17.8.1).

use std::collections::HashSet;

use wordcraft_doc::Document;

use crate::package::{EMBEDDED_PARTS, EmbeddedManifest};

/// Most font files taken from one document.
pub(crate) const MAX_FONTS: usize = 64;

/// Most bytes of font files copied out of one document: one part's limit, however many
/// references the font table makes (each reference is a copy; 64 of them to one large part
/// were 64 copies, #3692).
const MAX_FONT_BYTES: u64 = crate::package::MAX_PART;

/// The font table's references to embedded font files, one per style.
const EMBEDS: [&str; 4] = ["w:embedRegular", "w:embedBold", "w:embedItalic", "w:embedBoldItalic"];

/// A font file a document embeds, and the name its font table gives the font: the name the text
/// asks for, which a subset Word embeds doesn't carry inside (#2240).
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedFont {
    /// `w:font/@w:name`.
    pub name: String,
    /// The font file, de-obfuscated.
    pub data: Vec<u8>,
}

/// The font files `doc`'s font table embeds, de-obfuscated, in table order. Not checked to be
/// fonts: that is up to the font database. Empty for a document read from anything but a package
/// with a font table.
pub fn embedded_fonts(doc: &Document) -> Vec<EmbeddedFont> {
    fonts_within(doc, MAX_FONT_BYTES)
}

/// [`embedded_fonts`], copying at most `budget` bytes of font files.
fn fonts_within(doc: &Document, mut budget: u64) -> Vec<EmbeddedFont> {
    let budget_total = budget;
    let manifest = doc.passthrough.get(EMBEDDED_PARTS).map(|m| EmbeddedManifest::parse(m)).unwrap_or_default();
    // The table is kept under the spelling the package stores it as (#3872).
    let Some(key) = crate::package::font_table_key(&doc.passthrough) else {
        return Vec::new();
    };
    let Some(table) = doc.passthrough.get(key).and_then(|b| crate::xml::parse(b).ok()) else {
        return Vec::new();
    };
    let Some(part) = manifest.parts.get(key) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // A reference repeating one already taken (same file, key and name) gives the same font.
    let mut taken: HashSet<(&str, [u8; 16], &str)> = HashSet::new();
    let mut over_budget = 0usize;
    let embeds = table
        .children("w:font")
        .flat_map(|f| f.els().filter(|e| EMBEDS.contains(&e.name.as_str())).map(move |e| (f.attr("w:name").unwrap_or(""), e)));
    for (name, embed) in embeds {
        if out.len() >= MAX_FONTS {
            log::warn!("docx: more than {MAX_FONTS} embedded fonts; the rest are not loaded");
            break;
        }
        let Some(rel) = embed.attr("r:id").and_then(|id| part.rels.iter().find(|r| r.id == id && !r.external)) else {
            continue;
        };
        let Some(bytes) = doc.passthrough.get(&rel.target) else {
            continue;
        };
        // No key: the file is stored as is, as with the all-zero key.
        let key = match embed.attr("w:fontKey") {
            Some(k) => match font_key(k) {
                Some(key) => key,
                None => {
                    log::warn!("docx: embedded font {} has an unreadable key {k:?}", rel.target);
                    continue;
                }
            },
            None => [0; 16],
        };
        if !taken.insert((rel.target.as_str(), key, name)) {
            continue;
        }
        // Checked before the copy is made.
        let Some(left) = budget.checked_sub(bytes.len() as u64) else {
            over_budget += 1;
            continue;
        };
        budget = left;
        let mut data = bytes.to_vec();
        obfuscate(&mut data, &key);
        out.push(EmbeddedFont { name: name.to_string(), data });
    }
    if over_budget > 0 {
        // Once for the document, not once per font.
        log::warn!("docx: embedded fonts past the {budget_total} byte budget; {over_budget} of them are not loaded");
    }
    out
}

/// Obfuscate a font file with `key`, or undo it (the same XOR): the first 32 bytes, each with the
/// key's byte at its position modulo 16 (ECMA-376 Part 1, 17.8.1).
fn obfuscate(data: &mut [u8], key: &[u8; 16]) {
    for (i, b) in data.iter_mut().take(32).enumerate() {
        *b ^= key.get(i % 16).copied().unwrap_or(0);
    }
}

/// The obfuscation key of a `w:fontKey` GUID (`{XXXXXXXX-XXXX-…}`): its 16 bytes, reversed.
fn font_key(guid: &str) -> Option<[u8; 16]> {
    let digits: Vec<u8> = guid.bytes().filter(|c| !matches!(c, b'{' | b'}' | b'-')).collect();
    if digits.len() != 32 {
        return None;
    }
    let mut key = [0u8; 16];
    for (i, pair) in digits.chunks(2).enumerate() {
        let byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        *key.get_mut(15 - i)? = byte;
    }
    Some(key)
}

// --- Embedding fonts on save (File › Options › Save, "Embed fonts in the file") ---

/// The styles a font table embeds a font file for (ECMA-376 Part 1, 17.8.3.3–6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EmbedStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl EmbedStyle {
    /// Every style, in the order the font table lists them.
    pub const ALL: [EmbedStyle; 4] = [EmbedStyle::Regular, EmbedStyle::Bold, EmbedStyle::Italic, EmbedStyle::BoldItalic];

    /// The style with this weight and slant.
    pub fn of(bold: bool, italic: bool) -> EmbedStyle {
        match (bold, italic) {
            (false, false) => EmbedStyle::Regular,
            (true, false) => EmbedStyle::Bold,
            (false, true) => EmbedStyle::Italic,
            (true, true) => EmbedStyle::BoldItalic,
        }
    }

    /// The font table element that embeds a file for this style.
    pub(crate) fn element(self) -> &'static str {
        match self {
            EmbedStyle::Regular => "w:embedRegular",
            EmbedStyle::Bold => "w:embedBold",
            EmbedStyle::Italic => "w:embedItalic",
            EmbedStyle::BoldItalic => "w:embedBoldItalic",
        }
    }
}

/// A font file a save embeds for one style of a font: a plain TrueType or OpenType file, written
/// obfuscated under a fresh key.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFile {
    pub style: EmbedStyle,
    pub data: Vec<u8>,
}

/// Most font names a save asks the font provider about.
pub(crate) const MAX_EMBED_NAMES: usize = 256;
/// Longest font name a save asks about (a hostile file's names can be long).
const MAX_EMBED_NAME_CHARS: usize = 256;
/// Most bytes of font files one save embeds (each file once, however many styles share it).
pub(crate) const MAX_EMBED_BYTES: u64 = 128 << 20;

/// The fonts a save embeds: each font name with its files, at most one per style.
pub(crate) type FontsToEmbed = Vec<(String, Vec<FontFile>)>;

/// The names of the fonts the kept font table already embeds files for, and how many references
/// to font files it makes. Such a font is written back as it is, not embedded again.
pub(crate) fn kept_embeds(doc: &Document) -> (Vec<String>, usize) {
    let table = crate::package::font_table_key(&doc.passthrough).and_then(|k| doc.passthrough.get(k)).and_then(|b| crate::xml::parse(b).ok());
    let Some(table) = table else { return (Vec::new(), 0) };
    let mut names = Vec::new();
    let mut refs = 0usize;
    for f in table.children("w:font") {
        let n = f.els().filter(|e| EMBEDS.contains(&e.name.as_str())).count();
        if n > 0 {
            refs = refs.saturating_add(n);
            names.push(f.attr("w:name").unwrap_or("").to_string());
        }
    }
    (names, refs)
}

/// Ask `provider` for the files of each font in `names` that isn't in `skip`, within the limits:
/// at most `refs_left` references to font files (so [`embedded_fonts`] reads them all back) and
/// [`MAX_EMBED_BYTES`] of font data. One file per style; a font with none is left out.
pub(crate) fn fonts_to_embed(names: &[String], skip: &[String], mut refs_left: usize, provider: &dyn Fn(&str) -> Vec<FontFile>) -> FontsToEmbed {
    let mut out: FontsToEmbed = Vec::new();
    let mut bytes = 0u64;
    let mut left_out = 0usize;
    for name in names.iter().take(MAX_EMBED_NAMES) {
        if skip.iter().any(|s| s.trim().eq_ignore_ascii_case(name.trim())) || out.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        let mut files: Vec<FontFile> = Vec::new();
        for f in provider(name) {
            if f.data.is_empty() || files.iter().any(|g| g.style == f.style) {
                continue;
            }
            files.push(f);
        }
        if files.is_empty() {
            continue;
        }
        // A file shared by several styles (or fonts) is written once.
        let new_bytes: u64 = files
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                !files.get(..*i).unwrap_or(&[]).iter().any(|g| g.data == f.data) && !out.iter().flat_map(|(_, fs)| fs).any(|g| g.data == f.data)
            })
            .map(|(_, f)| f.data.len() as u64)
            .sum();
        if files.len() > refs_left || bytes.saturating_add(new_bytes) > MAX_EMBED_BYTES {
            left_out += 1;
            continue;
        }
        refs_left -= files.len();
        bytes = bytes.saturating_add(new_bytes);
        out.push((name.clone(), files));
    }
    if left_out > 0 {
        log::warn!("docx: {left_out} fonts are not embedded: past the limit of embedded fonts");
    }
    out
}

/// The font names the written parts ask for, each once (case-insensitively) in first-seen order:
/// `w:rFonts` and `w:sym` in WordprocessingML, `a:latin`, `a:ea` and `a:cs` typefaces in
/// DrawingML, and the theme's fonts that `*Theme` attributes and `+mn-lt`-style typefaces refer
/// to (from `theme`, the theme part written).
pub(crate) fn used_font_names<'a>(parts: impl Iterator<Item = &'a [u8]>, theme: Option<&[u8]>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let add = |names: &mut Vec<String>, n: &str| {
        let n: String = n.trim().chars().take(MAX_EMBED_NAME_CHARS).collect();
        if !n.is_empty() && !n.starts_with('+') && names.len() < MAX_EMBED_NAMES && !names.iter().any(|m| m.eq_ignore_ascii_case(&n)) {
            names.push(n);
        }
    };
    // Theme fonts referred to: (major?, slot: 0 latin, 1 East Asian, 2 complex script).
    let mut theme_refs: HashSet<(bool, u8)> = HashSet::new();
    for part in parts {
        let Ok(root) = crate::xml::parse(part) else { continue };
        let mut stack = vec![&root];
        while let Some(e) = stack.pop() {
            match e.name.as_str() {
                "w:rFonts" => {
                    for a in ["w:ascii", "w:hAnsi", "w:eastAsia", "w:cs"] {
                        if let Some(v) = e.attr(a) {
                            add(&mut names, v);
                        }
                    }
                    for a in ["w:asciiTheme", "w:hAnsiTheme", "w:eastAsiaTheme", "w:cstheme"] {
                        if let Some(r) = e.attr(a).and_then(theme_slot) {
                            theme_refs.insert(r);
                        }
                    }
                }
                "w:sym" => {
                    if let Some(v) = e.attr("w:font") {
                        add(&mut names, v);
                    }
                }
                "a:latin" | "a:ea" | "a:cs" => {
                    if let Some(v) = e.attr("typeface") {
                        match drawing_theme_slot(v) {
                            Some(r) => {
                                theme_refs.insert(r);
                            }
                            None => add(&mut names, v),
                        }
                    }
                }
                _ => {}
            }
            // In document order. The parser bounds the depth and the element count, so the stack
            // is bounded too.
            stack.extend(e.kids.iter().rev().filter_map(|n| match n {
                crate::xml::Node::El(c) => Some(c),
                crate::xml::Node::Text(_) => None,
            }));
        }
    }
    if theme_refs.is_empty() {
        return names;
    }
    let Some(scheme) = theme.and_then(|t| crate::xml::parse(t).ok()) else { return names };
    let Some(fonts) = scheme.find("a:fontScheme") else { return names };
    for (major, list) in [(true, "a:majorFont"), (false, "a:minorFont")] {
        let Some(list) = fonts.child(list) else { continue };
        for (slot, name) in [(0u8, "a:latin"), (1, "a:ea"), (2, "a:cs")] {
            if theme_refs.contains(&(major, slot))
                && let Some(v) = list.child(name).and_then(|e| e.attr("typeface"))
            {
                add(&mut names, v);
            }
        }
    }
    names
}

/// The theme font a `w:rFonts` theme attribute names (`majorHAnsi`, `minorBidi`…).
fn theme_slot(v: &str) -> Option<(bool, u8)> {
    let (major, rest) = match v.strip_prefix("major") {
        Some(r) => (true, r),
        None => (false, v.strip_prefix("minor")?),
    };
    let slot = match rest {
        "Ascii" | "HAnsi" => 0,
        "EastAsia" => 1,
        "Bidi" => 2,
        _ => return None,
    };
    Some((major, slot))
}

/// The theme font a DrawingML typeface refers to (`+mj-lt`, `+mn-ea`…).
fn drawing_theme_slot(v: &str) -> Option<(bool, u8)> {
    let (major, rest) = match v.strip_prefix("+mj-") {
        Some(r) => (true, r),
        None => (false, v.strip_prefix("+mn-")?),
    };
    let slot = match rest {
        "lt" => 0,
        "ea" => 1,
        "cs" => 2,
        _ => return None,
    };
    Some((major, slot))
}

/// A fresh `w:fontKey` GUID (`{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, version 4) and the key it
/// obfuscates with. Without secure random numbers the GUID comes from a hash of `data` and `salt`:
/// the obfuscation only marks the file as one not to install; it protects nothing.
pub(crate) fn new_font_key(data: &[u8], salt: u64) -> (String, [u8; 16]) {
    let mut g = [0u8; 16];
    if getrandom::fill(&mut g).is_err() {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(salt.to_le_bytes());
        h.update(data);
        for (b, d) in g.iter_mut().zip(h.finalize().iter()) {
            *b = *d;
        }
    }
    g[6] = (g[6] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    let hex = |r: std::ops::Range<usize>| g.get(r).unwrap_or(&[]).iter().map(|b| format!("{b:02X}")).collect::<String>();
    let guid = format!("{{{}-{}-{}-{}-{}}}", hex(0..4), hex(4..6), hex(6..8), hex(8..10), hex(10..16));
    // Through the reader's own parsing, so what is written is what reads back.
    let key = font_key(&guid).unwrap_or([0; 16]);
    (guid, key)
}

/// `data` obfuscated with `key`, as a font part holds it.
pub(crate) fn obfuscated(data: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let mut v = data.to_vec();
    obfuscate(&mut v, key);
    v
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::package::FONT_TABLE_PART;

    /// A document whose font table makes `refs` references (`(font name, part)`) to font parts
    /// `parts` (`(name, size)`).
    fn doc_with(refs: &[(&str, &str)], parts: &[(&str, usize)]) -> Document {
        let mut table = String::from(
            r#"<w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
        );
        let mut manifest = format!("P\t{FONT_TABLE_PART}\tapplication/xml\n");
        for (i, (font, part)) in refs.iter().enumerate() {
            table.push_str(&format!(r#"<w:font w:name="{font}"><w:embedRegular r:id="rId{i}"/></w:font>"#));
            manifest.push_str(&format!("R\t{FONT_TABLE_PART}\trId{i}\tfont\t{part}\tI\n"));
        }
        table.push_str("</w:fonts>");
        let mut doc = Document::default();
        doc.passthrough.insert(FONT_TABLE_PART.into(), Arc::new(table.into_bytes()));
        doc.passthrough.insert(EMBEDDED_PARTS.into(), Arc::new(manifest.into_bytes()));
        for (part, size) in parts {
            doc.passthrough.insert((*part).into(), Arc::new(vec![7u8; *size]));
        }
        doc
    }

    #[test]
    fn a_font_referenced_again_is_copied_once() {
        // Each reference was its own copy: the 64 references allowed to one 256 MB part were
        // 16 GB of copies (#3692).
        let refs = vec![("Body", "word/fonts/a.odttf"); MAX_FONTS];
        let fonts = embedded_fonts(&doc_with(&refs, &[("word/fonts/a.odttf", 1000)]));
        assert_eq!(fonts.len(), 1);
    }

    #[test]
    fn font_copies_stop_at_the_byte_budget() {
        let refs: Vec<(String, &str)> = (0..MAX_FONTS).map(|i| (format!("Font {i}"), "word/fonts/a.odttf")).collect();
        let refs: Vec<(&str, &str)> = refs.iter().map(|(n, p)| (n.as_str(), *p)).collect();
        let doc = doc_with(&refs, &[("word/fonts/a.odttf", 1000)]);
        let (fonts, warned) = crate::test_log::warnings(|| fonts_within(&doc, 3500));
        assert_eq!(fonts.len(), 3, "three copies fit");
        let budget: Vec<_> = warned.iter().filter(|w| w.contains("byte budget")).collect();
        assert_eq!(budget.len(), 1, "one warning for the document, not one per font: {warned:?}");
        assert!(budget.iter().all(|w| w.contains(&format!("{} of them", MAX_FONTS - 3))), "{budget:?}");
        assert_eq!(embedded_fonts(&doc).len(), MAX_FONTS, "well within the real budget");
    }

    #[test]
    fn a_written_key_reads_back_and_undoes_its_obfuscation() {
        let data: Vec<u8> = (0..100u8).collect();
        let (guid, key) = new_font_key(&data, 0);
        assert_eq!(guid.len(), 38, "{guid}");
        assert!(guid.starts_with('{') && guid.ends_with('}') && guid.matches('-').count() == 4, "{guid}");
        assert_eq!(guid.chars().nth(15), Some('4'), "a version 4 GUID: {guid}");
        assert_eq!(font_key(&guid), Some(key));
        let stored = obfuscated(&data, &key);
        assert_ne!(stored[..32], data[..32]);
        assert_eq!(stored[32..], data[32..]);
        let mut back = stored.clone();
        obfuscate(&mut back, &key);
        assert_eq!(back, data);
        assert_ne!(new_font_key(&data, 0).0, guid, "fresh each time");
    }

    #[test]
    fn used_font_names_come_from_runs_symbols_drawings_and_the_theme_fonts_referred_to() {
        let body = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><w:body>
            <w:p><w:r><w:rPr><w:rFonts w:ascii="Quill Serif" w:hAnsi="quill serif" w:eastAsia="Quill Mincho" w:cs="  Quill Naskh "/></w:rPr></w:r>
            <w:r><w:sym w:font="Quill Dingbats" w:char="F04A"/></w:r>
            <w:r><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:cstheme="minorBidi"/></w:rPr></w:r>
            <a:latin typeface="Quill Shape"/><a:ea typeface="+mn-ea"/><a:cs typeface=""/></w:p></w:body></w:document>"#;
        let theme = br#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:fontScheme name="x">
            <a:majorFont><a:latin typeface="Quill Display"/><a:ea typeface="Quill Major EA"/><a:cs typeface=""/></a:majorFont>
            <a:minorFont><a:latin typeface="Quill Text"/><a:ea typeface="Quill Minor EA"/><a:cs typeface="Quill Minor CS"/></a:minorFont>
            </a:fontScheme></a:themeElements></a:theme>"#;
        let names = used_font_names([&body[..], b"not xml <<<"].into_iter(), Some(theme));
        assert_eq!(
            names,
            ["Quill Serif", "Quill Mincho", "Quill Naskh", "Quill Dingbats", "Quill Shape", "Quill Display", "Quill Minor EA", "Quill Minor CS"]
        );
        // No references to the theme: its fonts aren't asked about.
        let plain = br#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:rFonts w:ascii="Quill Serif"/></w:p>"#;
        assert_eq!(used_font_names(std::iter::once(&plain[..]), Some(theme)), ["Quill Serif"]);
    }

    #[test]
    fn fonts_to_embed_keeps_each_font_whole_within_the_limits() {
        let names: Vec<String> = ["Kept", "A", "B", "c", "C"].iter().map(|s| s.to_string()).collect();
        let file = |style, seed: u8| FontFile { style, data: vec![seed; 10] };
        let provider = |name: &str| match name {
            "Kept" => vec![file(EmbedStyle::Regular, 1)],
            "A" => vec![file(EmbedStyle::Regular, 2), file(EmbedStyle::Bold, 3)],
            "B" => vec![file(EmbedStyle::Regular, 4), file(EmbedStyle::Bold, 5), file(EmbedStyle::Italic, 6)],
            "c" => vec![file(EmbedStyle::Regular, 7)],
            _ => panic!("{name} asked twice"),
        };
        // Three references left: A's two fit, B's three don't (left out whole), c's one does.
        let (out, warned) = crate::test_log::warnings(|| fonts_to_embed(&names, &[" kept ".into()], 3, &provider));
        let got: Vec<(&str, usize)> = out.iter().map(|(n, f)| (n.as_str(), f.len())).collect();
        assert_eq!(got, [("A", 2), ("c", 1)]);
        assert!(warned.iter().any(|w| w.contains("1 fonts are not embedded")), "{warned:?}");
    }
}
