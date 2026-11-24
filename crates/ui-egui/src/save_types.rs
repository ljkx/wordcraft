//! The file types Save As offers, in the order the dialog lists them.

use craft::host::FileFilter;
use wordcraft_engine::io::{SAVE_ALIASES, SAVE_EXTS};

/// The format a saved extension (lower case) belongs to: Markdown for `.markdown`, an alias's
/// format ([`SAVE_ALIASES`]: `.mhtml` is a Single File Web Page, `.mht`), else the extension.
fn format_of(ext: &str) -> &str {
    if ext == "markdown" {
        return "md";
    }
    SAVE_ALIASES.iter().find(|(alias, _)| *alias == ext).map_or(ext, |(_, format)| format)
}

/// The suggested name's extension, lower case, when the engine saves it (a format's own
/// extension, one of its [`SAVE_ALIASES`] or `.markdown`).
fn suggested_ext(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .filter(|e| SAVE_EXTS.contains(&format_of(e)))
}

/// The extensions Save As offers: every format the engine saves, with the suggested name's own
/// format first. A picker makes a filter's first extension the dialog's default, which the
/// panels append to a file name typed without one; with no filter at all the name came back bare
/// and the save failed, since the format is chosen by extension (#689).
fn save_exts(name: &str) -> Vec<String> {
    let suggested = suggested_ext(name).map(|e| format_of(&e).to_string()).unwrap_or_else(|| "docx".to_string());
    let mut exts: Vec<String> = Vec::with_capacity(SAVE_EXTS.len());
    exts.push(suggested.clone());
    exts.extend(SAVE_EXTS.iter().filter(|e| **e != suggested).map(|e| (*e).to_string()));
    exts
}

/// Every extension `format` saves under: its own, then its [`SAVE_ALIASES`] (a Single File Web
/// Page is `.mht` or `.mhtml`, as Word's Save As offers it), with `own` first when it is one of
/// them, so the dialog keeps the extension a name already has. The framework's Save dialog keeps
/// a typed extension that a filter lists, so `page.mhtml` stays `page.mhtml` with this type picked.
pub(crate) fn format_exts(format: &str, own: Option<&str>) -> Vec<String> {
    let mut exts = vec![format.to_string()];
    exts.extend(SAVE_ALIASES.iter().take(0).filter(|(_, f)| *f == format).map(|(alias, _)| (*alias).to_string()));
    if let Some(i) = own.and_then(|own| exts.iter().position(|e| e == own)) {
        let own = exts.remove(i);
        exts.insert(0, own);
    }
    exts
}

/// The "Save as type" list (#1399): one named entry per format, the suggested one first (see
/// [`save_exts`] for why the order matters). Markdown's entry also takes `.markdown`; a format
/// with [`SAVE_ALIASES`] lists them all ([`format_exts`]).
pub fn save_filters(name: &str) -> Vec<FileFilter> {
    let suggested = suggested_ext(name);
    save_exts(name)
        .into_iter()
        .map(|e| {
            if e == "md" {
                return FileFilter::new(format!("{} (*.md)", format_name(&e)), &["md", "markdown"]);
            }
            let exts = format_exts(&e, suggested.as_deref());
            let patterns: Vec<String> = exts.iter().map(|x| format!("*.{x}")).collect();
            let exts: Vec<&str> = exts.iter().map(String::as_str).collect();
            FileFilter::new(format!("{} ({})", format_name(&e), patterns.join("; ")), &exts)
        })
        .collect()
}

/// A readable name for a save format's extension.
fn format_name(ext: &str) -> String {
    match ext {
        "docx" => "Word Document".into(),
        "docm" => "Word Macro-Enabled Document".into(),
        "dotx" => "Word Template".into(),
        "dotm" => "Word Macro-Enabled Template".into(),
        "pdf" => "PDF".into(),
        "txt" => "Plain Text".into(),
        "md" => "Markdown".into(),
        "html" => "Web Page".into(),
        "mht" => "Single File Web Page".into(),
        "rtf" => "Rich Text Format".into(),
        "odt" => "OpenDocument Text".into(),
        "ott" => "OpenDocument Text Template".into(),
        "fodt" => "Flat OpenDocument Text".into(),
        "tex" => "LaTeX".into(),
        "png" => "PNG Image".into(),
        "json" => "JSON".into(),
        "xml" => "XML".into(),
        other => other.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_exts, save_filters};

    /// #1399, #689: the type list has one entry per format, the suggested name's own format first
    /// (a name typed without an extension then gets it), every savable extension exactly once.
    #[test]
    fn save_dialog_lists_one_named_type_per_format() {
        let filters = save_filters("report.odt");
        assert_eq!(filters.len(), wordcraft_engine::io::SAVE_EXTS.len());
        assert_eq!(filters[0].extensions, vec!["odt".to_string()], "the suggested format is first");
        let all: Vec<&String> = filters.iter().flat_map(|f| &f.extensions).collect();
        for e in wordcraft_engine::io::SAVE_EXTS {
            assert_eq!(all.iter().filter(|x| x.as_str() == *e).count(), 1, "{e}");
        }
        // A .markdown name is Markdown's own entry, first, and nothing lists the extension twice.
        let markdown = save_filters("notes.markdown");
        assert_eq!(markdown[0].extensions.first().map(String::as_str), Some("md"));
        assert!(markdown[0].extensions.iter().any(|e| e == "markdown"));
        let every: Vec<&String> = markdown.iter().flat_map(|f| &f.extensions).collect();
        assert_eq!(every.iter().filter(|x| x.as_str() == "markdown").count(), 1);
        // Unknown names fall back to Word's format first.
        assert_eq!(save_filters("no extension")[0].extensions, vec!["docx".to_string()]);
        assert_eq!(save_filters("mystery.rtf2")[0].extensions, vec!["docx".to_string()]);
    }

    /// A Single File Web Page is one entry for `.mht` and `.mhtml`, the suggested name's own first,
    /// so the dialog's default is the extension the name has. The framework's Save dialog keeps a
    /// typed extension any filter lists, so a typed `page.mhtml` isn't saved as `page.mhtml.mht`.
    #[test]
    fn a_single_file_web_page_is_one_type_for_both_extensions() {
        let s = |v: &[&str]| v.iter().map(|e| (*e).to_string()).collect::<Vec<_>>();
        let mht = &save_filters("page.mht")[0];
        assert_eq!((mht.name.as_str(), &mht.extensions), ("Single File Web Page (*.mht; *.mhtml)", &s(&["mht", "mhtml"])));
        let mhtml = &save_filters("page.MHTML")[0];
        assert_eq!((mhtml.name.as_str(), &mhtml.extensions), ("Single File Web Page (*.mhtml; *.mht)", &s(&["mhtml", "mht"])));
        let all = save_filters("report.docx");
        let pages: Vec<_> = all.iter().filter(|f| f.name.starts_with("Single File Web Page")).collect();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].extensions, s(&["mht", "mhtml"]));
        assert_eq!(all.iter().flat_map(|f| &f.extensions).filter(|e| *e == "mhtml").count(), 1);
        // File › Export's type: both extensions, the format's own first.
        assert_eq!(format_exts("mht", None), s(&["mht", "mhtml"]));
        assert_eq!(format_exts("pdf", Some("mhtml")), s(&["pdf"]));
    }
}
