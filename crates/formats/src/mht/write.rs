//! Saving a Single File Web Page (`.mht`, `.mhtml`): the page [`crate::html`] writes and its
//! pictures in one MIME message, as RFC 2557 describes (a `multipart/related` message whose
//! first part is the page, RFC 2387), written with craft-mime's writer.
//!
//! - The page is the root part, `text/html` in UTF-8 ([`craft_mime::Entity::text`]:
//!   quoted-printable unless it is ASCII in short lines), with the `Content-Location`
//!   [`PAGE_LOCATION`].
//! - Each distinct picture is a part of its own, base64, with the `Content-ID`
//!   `<imageNNN.ext>` and the `Content-Location` `file:///document_files/imageNNN.ext`. The page
//!   names it by its ID (`cid:imageNNN.ext`, RFC 2392), not by its location: Chrome and Edge
//!   show no picture whose part has a `file:` location, and Word turns a picture found only by a
//!   location at the root of `file:///` into a link to a missing file instead of keeping it. Both
//!   load `cid:` pictures, and Word keeps the file name the ID gives (`image1.png`). A picture
//!   shown twice is stored once.
//! - The document's title is the message's `Subject` ([`craft_mime::encode_unstructured`]).
//!
//! The output is the same bytes for the same document: no date, and craft-mime's boundary is a
//! hash of the parts, moved on when a part holds it. The work is linear in the size of the page.

use std::collections::HashMap;

use craft_mime::{Boundaries, Entity, transfer};
use wordcraft_doc::Document;

/// Where the page is (its `Content-Location`).
pub(super) const PAGE_LOCATION: &str = "file:///document.htm";
/// The folder beside the page that its pictures' locations put them in.
pub(super) const FILES: &str = "document_files";
/// What every boundary of the message starts with.
pub(super) const BOUNDARY_PREFIX: &str = "----=_WordCraft_Part_";
/// Most characters of the title written in the `Subject` header.
pub(super) const MAX_SUBJECT_CHARS: usize = 256;
/// How [`crate::html::export`] writes a picture: `<img src="data:<type>;base64,<data>"`.
const IMG_DATA: &str = "<img src=\"data:";

/// One picture part.
struct Picture<'a> {
    /// Its media type, as the page's `data:` URI gave it.
    mime: &'a str,
    /// Its bytes.
    data: Vec<u8>,
    /// Its file name in [`FILES`], which is also its `Content-ID`.
    name: String,
}

/// `doc` as a Single File Web Page: the page [`crate::html::export`] writes, with its pictures
/// as parts of their own, and the document's title as the message's subject.
pub fn export(doc: &Document) -> Vec<u8> {
    export_html(&crate::html::export(doc), &doc.core.title)
}

/// `html`, a page written by [`crate::html::export`] (with Print Markup's comment list, say), as
/// a Single File Web Page whose subject is `title`. Every picture the page holds as a base64
/// `data:` URI becomes a part the page names by its `Content-ID`; anything else stays as it is.
pub fn export_html(html: &str, title: &str) -> Vec<u8> {
    let (page, pictures) = take_pictures(html);
    let mut root = Entity::text("html", &page);
    root.fields.push(("Content-Location".into(), PAGE_LOCATION.into()));
    let mut parts = Vec::with_capacity(pictures.len().saturating_add(1));
    parts.push(root);
    parts.extend(pictures.iter().map(|p| Entity {
        fields: vec![
            ("Content-Type".into(), craft_mime::safe_media_type(p.mime)),
            ("Content-Transfer-Encoding".into(), "base64".into()),
            ("Content-ID".into(), format!("<{}>", craft_mime::clean_id(&p.name))),
            ("Content-Location".into(), format!("file:///{FILES}/{}", p.name)),
        ],
        content: transfer::base64_lines(&p.data),
    }));
    let message = Entity::multipart("related", &[("type", "text/html")], &parts, &mut Boundaries::new(BOUNDARY_PREFIX));
    let subject = subject(title);
    let headers: Vec<(&str, &str)> = subject.iter().map(|s| ("Subject", s.as_str())).collect();
    craft_mime::write_message(&headers, &message)
}

/// The page with each base64 `data:` picture replaced by `cid:` and its name, and the
/// pictures, each distinct one once, in the order the page first shows them.
///
/// Only [`crate::html::export`]'s own markup can hold `<img src="data:`: it escapes `<` and `"`
/// in text and attribute values, so text that reads like a picture stays text.
fn take_pictures(html: &str) -> (String, Vec<Picture<'_>>) {
    let mut page = String::with_capacity(html.len());
    let mut pictures: Vec<Picture<'_>> = Vec::new();
    // Keyed by the picture's base64 text, so a picture shown again is decoded once.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut rest = html;
    while let Some(at) = rest.find(IMG_DATA) {
        page.push_str(rest.get(..at).unwrap_or(""));
        // From `data:` to the closing quote.
        let uri_and_after = rest.get(at + IMG_DATA.len() - "data:".len()..).unwrap_or("");
        let uri = uri_and_after.find('"').and_then(|end| uri_and_after.get(..end).map(|uri| (uri, end)));
        let known = uri.and_then(|(uri, end)| {
            let (mime, base64) = data_uri(uri)?;
            if let Some(&i) = seen.get(base64) {
                return Some((i, end));
            }
            let data = craft_mime::base64::STANDARD.decode(base64).ok()?;
            let name = format!("image{:03}.{}", pictures.len() + 1, extension(mime));
            pictures.push(Picture { mime, data, name });
            let i = pictures.len() - 1;
            seen.insert(base64, i);
            Some((i, end))
        });
        match known {
            Some((i, end)) => {
                page.push_str("<img src=\"cid:");
                page.push_str(pictures.get(i).map(|p| p.name.as_str()).unwrap_or(""));
                // The closing quote and the rest of the tag follow.
                rest = uri_and_after.get(end..).unwrap_or("");
            }
            None => {
                page.push_str(IMG_DATA);
                rest = rest.get(at + IMG_DATA.len()..).unwrap_or("");
            }
        }
    }
    page.push_str(rest);
    (page, pictures)
}

/// The media type and base64 data of a `data:<type>;base64,<data>` URI, when the type is a
/// plain `type/subtype` token and the data is base64 (RFC 2397); `None` for anything else.
fn data_uri(uri: &str) -> Option<(&str, &str)> {
    let (mime, data) = uri.strip_prefix("data:")?.split_once(";base64,")?;
    let (kind, sub) = mime.split_once('/')?;
    let token = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b));
    let base64 = !data.is_empty() && data.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b));
    (token(kind) && token(sub) && base64).then_some((mime, data))
}

/// The file extension for a picture of media type `mime`.
fn extension(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/x-icon" => "ico",
        _ => "bin",
    }
}

/// The `Subject` value for `title`: its first [`MAX_SUBJECT_CHARS`] characters, control
/// characters (line breaks among them) as spaces, trimmed, as craft-mime encodes free text (as
/// it is when plain, else RFC 2047 encoded-words); `None` for an empty title.
pub(super) fn subject(title: &str) -> Option<String> {
    let clean: String = title.chars().take(usize::MAX).map(|c| c).collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| craft_mime::encode_unstructured("Subject", clean))
}

#[cfg(test)]
mod tests;
