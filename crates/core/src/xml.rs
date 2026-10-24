//! XML helpers shared by the Word and OpenDocument readers and writers.

/// Text content escaped for XML; characters XML cannot hold are dropped.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// An attribute value escaped for XML (double quotes).
pub fn esc_attr(s: &str) -> String {
    esc(s).replace('"', "&quot;")
}

/// The source text of an element, with every namespace declaration that is in scope written on it, so
/// the piece can be put into another document unchanged.
pub fn element_source(node: roxmltree::Node, src: &str) -> String {
    let range = node.range();
    let text = &src[range.clone()];
    let Some(end) = text.find(|c: char| c.is_whitespace() || c == '>' || c == '/') else { return text.to_string() };
    let own: Vec<String> = node.namespaces().map(|ns| match ns.name() {
        Some(p) => format!("xmlns:{p}"),
        None => "xmlns".to_string(),
    }).collect();
    let mut decl = String::new();
    for ns in node.namespaces() {
        let key = match ns.name() {
            Some(p) => format!("xmlns:{p}"),
            None => "xmlns".to_string(),
        };
        // skip what the start tag declares itself
        let start_tag_end = text.find('>').unwrap_or(text.len());
        if text[..start_tag_end].contains(&format!("{key}=")) {
            continue;
        }
        if own.contains(&key) {
            decl.push_str(&format!(" {}=\"{}\"", key, esc_attr(ns.uri())));
        }
    }
    format!("{}{}{}", &text[..end], decl, &text[end..])
}

/// Child elements with a local name in a namespace.
pub fn kids<'a, 'i>(node: roxmltree::Node<'a, 'i>, ns: &'a str, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> + 'a {
    node.children().filter(move |n| n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns))
}

pub fn kid<'a, 'i>(node: roxmltree::Node<'a, 'i>, ns: &str, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|n| n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns))
}

pub fn is(node: &roxmltree::Node, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(ns)
}

/// The first descendant element with a local name in a namespace.
pub fn find<'a, 'i>(node: roxmltree::Node<'a, 'i>, ns: &str, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.descendants().find(|n| is(n, ns, name))
}

pub fn attr<'a>(node: roxmltree::Node<'a, '_>, ns: &str, name: &str) -> Option<&'a str> {
    node.attribute((ns, name))
}

/// Decode the bytes of an XML part (UTF-8 or UTF-16) and drop a byte order mark.
pub fn decode(data: &[u8]) -> String {
    if data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]) {
        let le = data[0] == 0xFF;
        let units: Vec<u16> = data[2..].chunks(2).map(|c| if le { u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]) } else { u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) }).collect();
        return String::from_utf16_lossy(&units);
    }
    let s = String::from_utf8_lossy(data).into_owned();
    s.trim_start_matches('\u{feff}').to_string()
}
