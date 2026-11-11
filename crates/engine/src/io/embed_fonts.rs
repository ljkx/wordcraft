//! The font files a .docx save embeds when the document asks for it (File › Options › Save,
//! "Embed fonts in the file", #6329): for each font the document uses, the faces of it that are
//! installed, as Word embeds them (ECMA-376 Part 1, 17.8.1).

use wordcraft_docx::{EmbedStyle, FontFile};
use wordcraft_fonts::{FontFace, MAX_FONT_FILE_BYTES};

/// Fonts Windows and macOS install with the system, left out under "Do not embed common system
/// fonts" (the default): the computer opening the document has them already. Word doesn't
/// publish its own list; this is the fonts every current Windows and macOS has.
const COMMON_SYSTEM_FONTS: &[&str] = &[
    // Windows 10 and 11.
    "Arial",
    "Arial Black",
    "Bahnschrift",
    "Calibri",
    "Calibri Light",
    "Cambria",
    "Cambria Math",
    "Candara",
    "Comic Sans MS",
    "Consolas",
    "Constantia",
    "Corbel",
    "Courier New",
    "Ebrima",
    "Franklin Gothic Medium",
    "Gabriola",
    "Gadugi",
    "Georgia",
    "Impact",
    "Leelawadee UI",
    "Lucida Console",
    "Lucida Sans Unicode",
    "Malgun Gothic",
    "Marlett",
    "Microsoft Himalaya",
    "Microsoft JhengHei",
    "Microsoft New Tai Lue",
    "Microsoft PhagsPa",
    "Microsoft Sans Serif",
    "Microsoft Tai Le",
    "Microsoft YaHei",
    "Microsoft Yi Baiti",
    "MingLiU-ExtB",
    "Mongolian Baiti",
    "MS Gothic",
    "MS Mincho",
    "MS PGothic",
    "MS UI Gothic",
    "MV Boli",
    "Myanmar Text",
    "Nirmala UI",
    "NSimSun",
    "Palatino Linotype",
    "Segoe MDL2 Assets",
    "Segoe Print",
    "Segoe Script",
    "Segoe UI",
    "Segoe UI Emoji",
    "Segoe UI Historic",
    "Segoe UI Light",
    "Segoe UI Semibold",
    "Segoe UI Symbol",
    "SimSun",
    "SimSun-ExtB",
    "Sylfaen",
    "Symbol",
    "Tahoma",
    "Times New Roman",
    "Trebuchet MS",
    "Verdana",
    "Webdings",
    "Wingdings",
    "Yu Gothic",
    "Yu Mincho",
    // macOS.
    "Apple Color Emoji",
    "Apple Symbols",
    "Courier",
    "Geneva",
    "Helvetica",
    "Helvetica Neue",
    "Lucida Grande",
    "Menlo",
    "Monaco",
    "Times",
];

/// Is `font` one of the [`COMMON_SYSTEM_FONTS`]?
pub fn is_common_system_font(font: &str) -> bool {
    let font = font.trim();
    COMMON_SYSTEM_FONTS.iter().any(|f| f.eq_ignore_ascii_case(font))
}

/// The files a save embeds for `font`: for each style (Regular, Bold, Italic, Bold Italic), the
/// installed face the text in that style is drawn in, when it is the font's own (not a substitute,
/// not emboldened or slanted from another style) and may be embedded ([`embeddable`]). None for a
/// common system font when `skip_system` (Word's "Do not embed common system fonts").
pub(crate) fn files_for(font: &str, skip_system: bool) -> Vec<FontFile> {
    if skip_system && is_common_system_font(font) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for style in EmbedStyle::ALL {
        let (bold, italic) = match style {
            EmbedStyle::Regular => (false, false),
            EmbedStyle::Bold => (true, false),
            EmbedStyle::Italic => (false, true),
            EmbedStyle::BoldItalic => (true, true),
        };
        let r = wordcraft_fonts::word::resolve(font, bold, italic);
        if r.substituted || r.synth_bold || r.synth_italic {
            continue;
        }
        let face = r.face.get();
        if answers_to(face, font) && embeddable(face) {
            out.push(FontFile { style, data: face.data().to_vec() });
        }
    }
    out
}

/// Does `face` go by `font` (its family, legacy, full or PostScript name, or a localized family
/// name)? A face found any other way (a fallback list's other entry) isn't embedded under it.
fn answers_to(face: &FontFace, font: &str) -> bool {
    let font = font.trim();
    face.named(font) || face.local_names.iter().any(|l| l.name.eq_ignore_ascii_case(font))
}

/// Can `face` be embedded? Its licence must allow it (OpenType `fsType`: not Restricted License,
/// not bitmaps only), and it must be a single TrueType font file that reopens (at most
/// [`MAX_FONT_FILE_BYTES`], what the reader takes): Word embeds TrueType fonts, so OpenType fonts
/// with PostScript outlines and font collections are left out.
pub(crate) fn embeddable(face: &FontFace) -> bool {
    let data = face.data();
    let truetype = matches!(data.get(..4), Some([0, 1, 0, 0] | b"true"));
    if !truetype || face.index() != 0 || data.len() > MAX_FONT_FILE_BYTES {
        return false;
    }
    let licence = face.embedding();
    if !licence.allows_embedding() {
        log::info!("{} {} is not embedded: its licence doesn't allow it ({licence:?})", face.family, face.style);
        return false;
    }
    true
}
