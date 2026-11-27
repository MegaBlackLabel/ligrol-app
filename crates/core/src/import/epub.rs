//! EPUB（ZIP + OPF + XHTML）から章単位の本文を取り出す。
//!
//! 目的は**AI / 検索用の本文**であり、EPUB のレンダリングではない。spine の順に
//! XHTML を読み、ブロック境界・ルビ・コードブロックを保ってテキスト化する。
//!
//! 固定レイアウト（`rendition:layout = pre-paginated`）と DRM は本文を返さない。
//! ZIP ボム対策として展開サイズに上限を置く。

use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum EpubError {
    #[error("epub io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("epub zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("epub xml error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("epub is too large")]
    TooLarge,
    #[error("invalid epub: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpubSection {
    /// 1-indexed。spine の順。
    pub ordinal: i64,
    pub title: Option<String>,
    /// OPF からの相対パス（例 `OEBPS/ch01.xhtml`）。
    pub href: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EpubText {
    pub title: Option<String>,
    pub author: Option<String>,
    pub fixed_layout: bool,
    pub drm_protected: bool,
    pub sections: Vec<EpubSection>,
}

#[derive(Debug, Clone)]
struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

#[derive(Debug, Clone)]
struct SpineItem {
    idref: String,
    linear: bool,
}

#[derive(Debug, Default)]
struct Opf {
    title: Option<String>,
    author: Option<String>,
    fixed_layout: bool,
    items: Vec<ManifestItem>,
    spine: Vec<SpineItem>,
}

fn read_entry(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>, EpubError> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| EpubError::Invalid(format!("missing entry: {name}")))?;
    if file.size() > MAX_ENTRY_BYTES {
        return Err(EpubError::TooLarge);
    }
    let mut out = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut out)?;
    Ok(out)
}

fn decode_xml_text(value: &[u8]) -> String {
    String::from_utf8_lossy(value).into_owned()
}

fn unescape_text(value: &[u8]) -> String {
    let raw = decode_xml_text(value);
    // quick-xml の unescape は XML の 5 実体だけ。EPUB は HTML 実体を混ぜるので
    // よく使うものだけ追加で戻す（失敗しても本文を失わない）。
    let decoded = quick_xml::escape::unescape(&raw)
        .map(|value| value.into_owned())
        .unwrap_or(raw);
    decoded
        .replace("&nbsp;", " ")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…")
        .replace("&copy;", "©")
}

fn normalize_href(base_dir: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let decoded = percent_encoding::percent_decode_str(href)
        .decode_utf8_lossy()
        .into_owned();
    let mut parts: Vec<&str> = base_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

fn parse_container(xml: &[u8]) -> Result<String, EpubError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == b"rootfile" {
                    for attr in event.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"full-path" {
                            let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default();
                            return Ok(value.into_owned());
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(EpubError::Xml(error)),
            _ => {}
        }
        buf.clear();
    }
    Err(EpubError::Invalid("container.xml has no rootfile".into()))
}

fn parse_opf(xml: &[u8]) -> Result<Opf, EpubError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut opf = Opf::default();
    let mut capture: Option<&'static str> = None;
    let mut capture_text = String::new();
    let mut in_manifest = false;
    let mut in_spine = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = event.local_name();
                match name.as_ref() {
                    b"manifest" => in_manifest = true,
                    b"spine" => in_spine = true,
                    b"title" if !in_manifest && !in_spine => {
                        capture = Some("title");
                        capture_text.clear();
                    }
                    b"creator" if !in_manifest && !in_spine => {
                        capture = Some("creator");
                        capture_text.clear();
                    }
                    b"meta" if !in_manifest && !in_spine => {
                        let property = event
                            .attributes()
                            .flatten()
                            .find(|attr| attr.key.local_name().as_ref() == b"property")
                            .and_then(|attr| attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                            .map(|value| value.into_owned());
                        if property.as_deref() == Some("rendition:layout") {
                            capture = Some("layout");
                            capture_text.clear();
                        }
                    }
                    b"item" if in_manifest => {
                        let mut id = String::new();
                        let mut href = String::new();
                        let mut media_type = String::new();
                        let mut properties = String::new();
                        for attr in event.attributes().flatten() {
                            let key = attr.key.local_name();
                            let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                            match key.as_ref() {
                                b"id" => id = value,
                                b"href" => href = value,
                                b"media-type" => media_type = value,
                                b"properties" => properties = value,
                                _ => {}
                            }
                        }
                        if !href.is_empty() {
                            opf.items.push(ManifestItem {
                                id,
                                href,
                                media_type,
                                properties,
                            });
                        }
                    }
                    b"itemref" if in_spine => {
                        let mut idref = String::new();
                        let mut linear = true;
                        for attr in event.attributes().flatten() {
                            let key = attr.key.local_name();
                            let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                            match key.as_ref() {
                                b"idref" => idref = value,
                                b"linear" => linear = value != "no",
                                _ => {}
                            }
                        }
                        if !idref.is_empty() {
                            opf.spine.push(SpineItem { idref, linear });
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => {
                let name = event.local_name();
                if name.as_ref() == b"itemref" && in_spine {
                    let mut idref = String::new();
                    let mut linear = true;
                    for attr in event.attributes().flatten() {
                        let key = attr.key.local_name();
                        let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                        match key.as_ref() {
                            b"idref" => idref = value,
                            b"linear" => linear = value != "no",
                            _ => {}
                        }
                    }
                    if !idref.is_empty() {
                        opf.spine.push(SpineItem { idref, linear });
                    }
                } else if name.as_ref() == b"meta" && !in_manifest && !in_spine {
                    let property = event
                        .attributes()
                        .flatten()
                        .find(|attr| attr.key.local_name().as_ref() == b"property")
                        .and_then(|attr| attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                        .map(|value| value.into_owned());
                    if property.as_deref() == Some("rendition:layout") {
                        // 値は属性ではなく本文なので、この形は稀。何もしない。
                    }
                } else if name.as_ref() == b"item" && in_manifest {
                    // 空要素の manifest item は通常無いが、属性だけ拾う。
                    let mut id = String::new();
                    let mut href = String::new();
                    let mut media_type = String::new();
                    let mut properties = String::new();
                    for attr in event.attributes().flatten() {
                        let key = attr.key.local_name();
                        let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                        match key.as_ref() {
                            b"id" => id = value,
                            b"href" => href = value,
                            b"media-type" => media_type = value,
                            b"properties" => properties = value,
                            _ => {}
                        }
                    }
                    if !href.is_empty() {
                        opf.items.push(ManifestItem {
                            id,
                            href,
                            media_type,
                            properties,
                        });
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(kind) = capture {
                    let text = unescape_text(event.as_ref());
                    if !text.is_empty() {
                        if !capture_text.is_empty() {
                            capture_text.push(' ');
                        }
                        capture_text.push_str(text.trim());
                    }
                    let _ = kind;
                }
            }
            Ok(Event::End(event)) => {
                let name = event.local_name();
                match name.as_ref() {
                    b"manifest" => in_manifest = false,
                    b"spine" => in_spine = false,
                    b"title" if capture == Some("title") => {
                        if opf.title.is_none() {
                            opf.title = Some(capture_text.trim().to_string());
                        }
                        capture = None;
                    }
                    b"creator" if capture == Some("creator") => {
                        if opf.author.is_none() {
                            opf.author = Some(capture_text.trim().to_string());
                        }
                        capture = None;
                    }
                    b"meta" if capture == Some("layout") => {
                        if capture_text.trim() == "pre-paginated" {
                            opf.fixed_layout = true;
                        }
                        capture = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(EpubError::Xml(error)),
            _ => {}
        }
        buf.clear();
    }
    if opf.title.as_deref() == Some("") {
        opf.title = None;
    }
    if opf.author.as_deref() == Some("") {
        opf.author = None;
    }
    Ok(opf)
}

fn parse_nav(xml: &[u8]) -> Vec<(String, String)> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut in_nav = false;
    let mut in_anchor = false;
    let mut anchor_href = String::new();
    let mut anchor_text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = event.local_name();
                if name.as_ref() == b"nav" {
                    in_nav = true;
                } else if name.as_ref() == b"a" && in_nav {
                    in_anchor = true;
                    anchor_href.clear();
                    anchor_text.clear();
                    for attr in event.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"href" {
                            anchor_href = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                        }
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == b"a" && in_nav {
                    let mut href = String::new();
                    for attr in event.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"href" {
                            href = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                        }
                    }
                    if !href.is_empty() {
                        out.push((href, String::new()));
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if in_anchor {
                    anchor_text.push_str(&unescape_text(event.as_ref()));
                }
            }
            Ok(Event::End(event)) => {
                let name = event.local_name();
                if name.as_ref() == b"nav" {
                    in_nav = false;
                } else if name.as_ref() == b"a" && in_anchor {
                    if !anchor_href.is_empty() {
                        out.push((anchor_href.clone(), anchor_text.trim().to_string()));
                    }
                    in_anchor = false;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_ncx(xml: &[u8]) -> Vec<(String, String)> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut last_text = String::new();
    let mut in_text = false;
    let mut text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => match event.local_name().as_ref() {
                b"text" => {
                    in_text = true;
                    text.clear();
                }
                b"content" => {
                    for attr in event.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"src" {
                            let src = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                            if !src.is_empty() {
                                out.push((src, last_text.clone()));
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == b"content" {
                    for attr in event.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"src" {
                            let src = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).unwrap_or_default().into_owned();
                            if !src.is_empty() {
                                out.push((src, last_text.clone()));
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if in_text {
                    text.push_str(&unescape_text(event.as_ref()));
                }
            }
            Ok(Event::End(event)) => {
                if event.local_name().as_ref() == b"text" {
                    in_text = false;
                    last_text = text.trim().to_string();
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn is_block(name: &[u8]) -> bool {
    matches!(
        name,
        b"p" | b"div"
            | b"section"
            | b"article"
            | b"li"
            | b"tr"
            | b"blockquote"
            | b"pre"
            | b"figure"
            | b"figcaption"
            | b"table"
            | b"ul"
            | b"ol"
            | b"h1"
            | b"h2"
            | b"h3"
            | b"h4"
            | b"h5"
            | b"h6"
    )
}

fn append_block(out: &mut String) {
    if !out.ends_with("\n\n") {
        if out.ends_with('\n') {
            out.push('\n');
        } else {
            out.push_str("\n\n");
        }
    }
}

fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0usize;
    for line in text.replace('\r', "").lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn extract_xhtml(xml: &[u8]) -> Result<(String, Option<String>), EpubError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut skip_depth = 0usize;
    let mut pre_depth = 0usize;
    let mut heading_level = 0u8;
    let mut heading_text = String::new();
    let mut first_heading: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = event.local_name();
                if matches!(
                    name.as_ref(),
                    b"script" | b"style" | b"head" | b"rt" | b"rp" | b"nav"
                ) {
                    skip_depth += 1;
                } else if skip_depth == 0 {
                    match name.as_ref() {
                        b"br" => out.push('\n'),
                        b"li" => {
                            append_block(&mut out);
                            out.push_str("- ");
                        }
                        b"td" | b"th" => out.push('\t'),
                        b"pre" => {
                            pre_depth += 1;
                            append_block(&mut out);
                        }
                        b"h1" | b"h2" | b"h3" => {
                            heading_level = name.as_ref()[1] - b'0';
                            heading_text.clear();
                            append_block(&mut out);
                        }
                        _ if is_block(name.as_ref()) => append_block(&mut out),
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                let name = event.local_name();
                if skip_depth == 0 {
                    match name.as_ref() {
                        b"br" => out.push('\n'),
                        b"img" => {
                            for attr in event.attributes().flatten() {
                                if attr.key.local_name().as_ref() == b"alt"
                                    && let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                {
                                    let value = value.trim();
                                    if !value.is_empty() {
                                        out.push_str(&format!("[{}]", value));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if skip_depth == 0 {
                    let text = unescape_text(event.as_ref());
                    if pre_depth > 0 {
                        out.push_str(&text);
                    } else {
                        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !collapsed.is_empty() {
                            if !out.is_empty()
                                && !out.chars().last().is_some_and(char::is_whitespace)
                                && !matches!(
                                    collapsed.chars().next(),
                                    Some(
                                        '。' | '、' | '）' | '」' | '』' | '！' | '？' | ',' | '.'
                                    )
                                )
                            {
                                // 日本語のインライン要素間に余計な空白を入れない。
                                let prev = out.chars().last().unwrap_or(' ');
                                let next = collapsed.chars().next().unwrap_or(' ');
                                if prev.is_ascii_alphanumeric() && next.is_ascii_alphanumeric() {
                                    out.push(' ');
                                }
                            }
                            out.push_str(&collapsed);
                        }
                    }
                    if heading_level > 0 && first_heading.is_none() {
                        heading_text.push_str(&text);
                    }
                }
            }
            Ok(Event::CData(event)) => {
                if skip_depth == 0 {
                    out.push_str(&decode_xml_text(event.as_ref()));
                }
            }
            Ok(Event::End(event)) => {
                let name = event.local_name();
                if matches!(
                    name.as_ref(),
                    b"script" | b"style" | b"head" | b"rt" | b"rp" | b"nav"
                ) {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if skip_depth == 0 {
                    match name.as_ref() {
                        b"pre" => {
                            pre_depth = pre_depth.saturating_sub(1);
                            append_block(&mut out);
                        }
                        b"h1" | b"h2" | b"h3" => {
                            if first_heading.is_none() {
                                let value = heading_text
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                if !value.is_empty() {
                                    first_heading = Some(value);
                                }
                            }
                            heading_level = 0;
                            append_block(&mut out);
                        }
                        _ if is_block(name.as_ref()) => append_block(&mut out),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(EpubError::Xml(error)),
            _ => {}
        }
        buf.clear();
    }
    Ok((clean_text(&out), first_heading))
}

/// EPUB のバイト列から章本文を取り出す。
pub fn extract(bytes: &[u8]) -> Result<EpubText, EpubError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        total = total.saturating_add(file.size());
    }
    if total > MAX_TOTAL_BYTES {
        return Err(EpubError::TooLarge);
    }

    let drm_protected = match read_entry(&mut archive, "META-INF/encryption.xml") {
        Ok(xml) => {
            let xml = String::from_utf8_lossy(&xml).to_ascii_lowercase();
            xml.contains("encrypteddata") && (xml.contains(".xhtml") || xml.contains(".html"))
        }
        Err(_) => false,
    };

    let container = read_entry(&mut archive, "META-INF/container.xml")?;
    let opf_path = parse_container(&container)?;
    let opf_dir = parent_dir(&opf_path).to_string();
    let opf_xml = read_entry(&mut archive, &opf_path)?;
    let opf = parse_opf(&opf_xml)?;

    if drm_protected {
        return Ok(EpubText {
            title: opf.title,
            author: opf.author,
            fixed_layout: opf.fixed_layout,
            drm_protected: true,
            sections: Vec::new(),
        });
    }

    // TOC（EPUB3 nav / EPUB2 NCX）を href -> ラベルに。
    let mut toc: Vec<(String, String)> = Vec::new();
    if let Some(nav) = opf.items.iter().find(|item| {
        item.properties
            .split_whitespace()
            .any(|value| value == "nav")
    }) {
        let path = normalize_href(&opf_dir, &nav.href);
        if let Ok(xml) = read_entry(&mut archive, &path) {
            let base = parent_dir(&path);
            toc.extend(
                parse_nav(&xml)
                    .into_iter()
                    .map(|(href, label)| (normalize_href(base, &href), label)),
            );
        }
    }
    if let Some(ncx) = opf
        .items
        .iter()
        .find(|item| item.media_type == "application/x-dtbncx+xml")
    {
        let path = normalize_href(&opf_dir, &ncx.href);
        if let Ok(xml) = read_entry(&mut archive, &path) {
            let base = parent_dir(&path);
            toc.extend(
                parse_ncx(&xml)
                    .into_iter()
                    .map(|(href, label)| (normalize_href(base, &href), label)),
            );
        }
    }
    let toc_map: std::collections::HashMap<String, String> = toc
        .into_iter()
        .filter(|(_, label)| !label.is_empty())
        .collect();

    let mut sections = Vec::new();
    for spine in &opf.spine {
        if !spine.linear {
            continue;
        }
        let Some(item) = opf.items.iter().find(|item| item.id == spine.idref) else {
            continue;
        };
        if !item.media_type.contains("xhtml") && !item.media_type.contains("html") {
            continue;
        }
        let path = normalize_href(&opf_dir, &item.href);
        let Ok(xml) = read_entry(&mut archive, &path) else {
            continue;
        };
        let (text, heading) = extract_xhtml(&xml)?;
        if text.is_empty() {
            continue;
        }
        let title = toc_map
            .get(&path)
            .cloned()
            .filter(|value| !value.is_empty())
            .or(heading);
        sections.push(EpubSection {
            ordinal: sections.len() as i64 + 1,
            title,
            href: path,
            text,
        });
    }

    Ok(EpubText {
        title: opf.title,
        author: opf.author,
        fixed_layout: opf.fixed_layout,
        drm_protected: false,
        sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn epub_bytes(chapters: &[(&str, &str)], fixed: bool, drm: bool, ncx: bool) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("mimetype", options).unwrap();
            zip.write_all(b"application/epub+zip").unwrap();
            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();
            if drm {
                zip.start_file("META-INF/encryption.xml", options).unwrap();
                zip.write_all(br#"<encryption><EncryptedData><CipherReference URI="OEBPS/ch1.xhtml"/></EncryptedData></encryption>"#)
                    .unwrap();
            }
            zip.start_file("OEBPS/content.opf", options).unwrap();
            let layout = if fixed {
                r#"<meta property="rendition:layout">pre-paginated</meta>"#
            } else {
                ""
            };
            let ncx_item = if ncx {
                r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#
            } else {
                r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#
            };
            let mut opf = format!(
                r#"<package><metadata><dc:title>テスト本</dc:title><dc:creator>著者</dc:creator>{layout}</metadata><manifest>{ncx_item}"#
            );
            for (index, _) in chapters.iter().enumerate() {
                opf.push_str(&format!(
                    r#"<item id="ch{index}" href="ch{index}.xhtml" media-type="application/xhtml+xml"/>"#
                ));
            }
            opf.push_str("</manifest><spine>");
            for (index, _) in chapters.iter().enumerate() {
                opf.push_str(&format!(r#"<itemref idref="ch{index}"/>"#));
            }
            opf.push_str("</spine></package>");
            zip.write_all(opf.as_bytes()).unwrap();
            if ncx {
                zip.start_file("OEBPS/toc.ncx", options).unwrap();
                let mut ncx = String::from("<ncx><navMap>");
                for (index, (title, _)) in chapters.iter().enumerate() {
                    ncx.push_str(&format!(
                        r#"<navPoint><navLabel><text>{title}</text></navLabel><content src="ch{index}.xhtml"/></navPoint>"#
                    ));
                }
                ncx.push_str("</navMap></ncx>");
                zip.write_all(ncx.as_bytes()).unwrap();
            } else {
                zip.start_file("OEBPS/nav.xhtml", options).unwrap();
                let mut nav = String::from(r#"<html><body><nav epub:type="toc">"#);
                for (index, (title, _)) in chapters.iter().enumerate() {
                    nav.push_str(&format!(r#"<a href="ch{index}.xhtml">{title}</a>"#));
                }
                nav.push_str("</nav></body></html>");
                zip.write_all(nav.as_bytes()).unwrap();
            }
            for (index, body) in chapters.iter().map(|(_, body)| body).enumerate() {
                zip.start_file(format!("OEBPS/ch{index}.xhtml"), options)
                    .unwrap();
                zip.write_all(format!("<html><body>{body}</body></html>").as_bytes())
                    .unwrap();
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn extracts_spine_order_titles_and_ruby() {
        let data = epub_bytes(
            &[
                (
                    "第1章",
                    "<h1>第1章</h1><p>本文<ruby>漢<rt>かん</rt></ruby>です。</p>",
                ),
                ("第2章", "<h1>第2章</h2><pre>let x = 1;</pre><p>二章</p>"),
            ],
            false,
            false,
            false,
        );
        let epub = extract(&data).unwrap();
        assert_eq!(epub.title.as_deref(), Some("テスト本"));
        assert_eq!(epub.sections.len(), 2);
        assert_eq!(epub.sections[0].title.as_deref(), Some("第1章"));
        assert!(epub.sections[0].text.contains("本文漢です。"));
        assert!(!epub.sections[0].text.contains("かん"));
        assert!(epub.sections[1].text.contains("let x = 1;"));
    }

    #[test]
    fn detects_drm_and_fixed_layout() {
        let drm = epub_bytes(&[("ch", "<p>秘密</p>")], false, true, false);
        let epub = extract(&drm).unwrap();
        assert!(epub.drm_protected);
        assert!(epub.sections.is_empty());

        let fixed = epub_bytes(&[("ch", "<p>固定</p>")], true, false, false);
        let epub = extract(&fixed).unwrap();
        assert!(epub.fixed_layout);
        assert_eq!(epub.sections.len(), 1);
    }

    #[test]
    fn parses_ncx_titles() {
        let data = epub_bytes(&[("NCX 章", "<p>本文</p>")], false, false, true);
        let epub = extract(&data).unwrap();
        assert_eq!(epub.sections[0].title.as_deref(), Some("NCX 章"));
    }

    #[test]
    fn import_epub_writes_sections_to_document_text() {
        crate::secrets::SecretStore::use_memory_backend();
        let pool = crate::db::test_pool();
        let dir = std::env::temp_dir().join(format!("ligrol-epub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = epub_bytes(
            &[
                ("第1章", "<h1>第1章</h1><p>本文です。</p>"),
                ("第2章", "<p>二章です。</p>"),
            ],
            false,
            false,
            false,
        );
        let imported =
            crate::import::import_epub_bytes(&pool, "test.epub", &data, &dir, None, None, None)
                .unwrap();
        let key = crate::db::column_crypto::db_key().unwrap();
        let rows = crate::db::documents::all_text_rows_for_document(
            &pool,
            &imported.document.id,
            Some(&key),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "epub");
        assert_eq!(rows[0].section_title.as_deref(), Some("第1章"));
        assert!(rows[0].text.as_deref().unwrap().contains("本文です。"));
        assert!(
            rows[1]
                .section_href
                .as_deref()
                .unwrap()
                .ends_with("ch1.xhtml")
        );
        let outcome = backfill_from_pack(&pool, &dir, None, &imported.book.id).unwrap();
        assert_eq!(outcome.sections, 2);
        assert_eq!(outcome.chars, 15);
        let rows =
            crate::db::documents::all_text_rows_for_document(&pool, &imported.document.id, Some(&key))
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "epub");
        assert_eq!(rows[0].section_title.as_deref(), Some("第1章"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 既存の EPUB 本（raw `.epub` を pack に持つ）から本文を遡及抽出する。
///
/// 取り込み時に抽出済みの本では `document_text(source='epub')` を入れ替える。
/// pack が見つからない / EPUB が入っていない場合はエラー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillOutcome {
    pub sections: i64,
    pub chars: i64,
    pub drm_protected: bool,
    pub fixed_layout: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum BackfillError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("pack error: {0}")]
    Pack(#[from] nartik::PackError),
    #[error("epub error: {0}")]
    Epub(#[from] EpubError),
    #[error("book is not EPUB")]
    NotEpub,
    #[error("pack has no EPUB entry")]
    NoEntry,
    #[error("book/document not found")]
    NotFound,
}

pub fn backfill_from_pack(
    pool: &crate::db::SqlitePool,
    packs_dir: &std::path::Path,
    root_key: Option<&nartik::PackRootKey>,
    book_id: &str,
) -> Result<BackfillOutcome, BackfillError> {
    let book = crate::db::books::get(pool, book_id)?.ok_or(BackfillError::NotFound)?;
    let document = crate::db::documents::get_document_by_book_id(pool, book_id)?
        .ok_or(BackfillError::NotFound)?;
    if document.source_type != "epub" {
        return Err(BackfillError::NotEpub);
    }
    let pack_id = book.pack_id.as_deref().unwrap_or(&book.id);
    let path = crate::pack_path::pack_path(packs_dir, pack_id)
        .map_err(|error| BackfillError::Pack(nartik::PackError::Corrupted(error.to_string())))?;
    let reader = nartik::PackFileReader::open(&path)?;
    let key = root_key.map(|root| root.derive_pack_key(pack_id));
    let entry = reader
        .entries()
        .iter()
        .find(|entry| entry.path.to_ascii_lowercase().ends_with(".epub"))
        .or_else(|| {
            reader
                .entries()
                .iter()
                .find(|entry| entry.mime_type == "application/epub+zip")
        })
        .ok_or(BackfillError::NoEntry)?;
    let bytes = reader.read_entry_with_key(&entry.path, key.as_ref())?;
    let parsed = extract(&bytes)?;
    let content_id = crate::db::block_on(async {
        sqlx::query_scalar::<_, String>(
            "SELECT content_id FROM book_contents WHERE book_id = ?1 \
             ORDER BY is_primary DESC, sort_order, content_id LIMIT 1",
        )
        .bind(book_id)
        .fetch_optional(pool)
        .await
    })?
    .unwrap_or_default();

    crate::db::block_on(async {
        sqlx::query("DELETE FROM document_text WHERE document_id = ?1 AND source = 'epub'")
            .bind(&document.id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM token_analysis WHERE document_id = ?1")
            .bind(&document.id)
            .execute(pool)
            .await?;
        Ok::<_, sqlx::Error>(())
    })?;

    if parsed.drm_protected {
        return Ok(BackfillOutcome {
            sections: 0,
            chars: 0,
            drm_protected: true,
            fixed_layout: parsed.fixed_layout,
        });
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let rows: Vec<crate::db::documents::DocumentText> = parsed
        .sections
        .iter()
        .map(|section| crate::db::documents::DocumentText {
            id: uuid::Uuid::new_v4().to_string(),
            document_id: document.id.clone(),
            content_id: content_id.clone(),
            page_number: section.ordinal,
            text_content: section.text.clone(),
            source: "epub".to_string(),
            engine: Some("epub-xhtml".to_string()),
            confidence: None,
            section_title: section.title.clone(),
            section_href: Some(section.href.clone()),
            created_at: timestamp.clone(),
        })
        .collect();
    let chars = rows
        .iter()
        .map(|row| row.text_content.chars().count() as i64)
        .sum();
    if !rows.is_empty() {
        crate::db::documents::insert_texts_batch(pool, &rows)?;
        let mut tokens = Vec::new();
        for row in &rows {
            for (word, count) in crate::tags::extract_nouns(&row.text_content, &[&book.title]) {
                tokens.push(crate::db::documents::TokenRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    document_id: document.id.clone(),
                    page_number: row.page_number,
                    token: word,
                    pos: "名詞".to_string(),
                    base_form: None,
                    reading: None,
                    frequency: count as i64,
                    created_at: timestamp.clone(),
                });
            }
        }
        if !tokens.is_empty() {
            crate::db::documents::insert_tokens_batch(pool, &tokens)?;
        }
    }
    Ok(BackfillOutcome {
        sections: rows.len() as i64,
        chars,
        drm_protected: false,
        fixed_layout: parsed.fixed_layout,
    })
}
