//! Embedded XMP metadata of a JP2 file (Adobe XMP specification, part 3: JPEG 2000).
//!
//! XMP lives in a top-level `uuid` box whose first 16 bytes are [`XMP_UUID`] and whose remainder is
//! an XML packet. Reading it only walks the top-level boxes, so on a memory-mapped multi-gigabyte
//! file it touches a handful of small regions (it does not read the codestream). Every read is
//! bounds-checked: truncated or damaged files yield an error or no metadata, never a panic.
//!
//! The reader is generic: it reports whatever simple properties the packet holds, in document
//! order, whatever their namespaces. Nothing here knows about any particular producer's fields.

use std::path::Path;

use memmap2::Mmap;
use roxmltree::{Document, Node, ParsingOptions};

use crate::inspect::boxes;

/// The UUID that marks an XMP packet box (XMP specification, part 3).
pub const XMP_UUID: [u8; 16] = [
    0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC,
];

/// Largest XMP packet we will parse. Real packets are a few KB.
const MAX_PACKET: usize = 16 * 1024 * 1024;

/// Width the panel and `tools meta` wrap their text to.
pub const XMP_COLUMNS: usize = 96;

/// Longest key shown in the key column; longer keys get a line of their own.
const MAX_KEY_COLUMN: usize = 32;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
const META_NS: &str = "adobe:ns:meta/";

/// One simple XMP property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmpProperty {
    /// Full namespace URI, e.g. `http://purl.org/dc/elements/1.1/`.
    pub namespace: String,
    /// Prefix as written in the packet (display only; match on `namespace`).
    pub prefix: String,
    /// Local name.
    pub name: String,
    /// Unescaped text.
    pub value: String,
}

/// The properties of a file, in document order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XmpMetadata {
    pub properties: Vec<XmpProperty>,
}

impl XmpMetadata {
    /// Add `props`; a property that already exists (same namespace and name) takes the new value
    /// but keeps its original position.
    fn merge(&mut self, props: Vec<XmpProperty>) {
        for p in props {
            match self
                .properties
                .iter_mut()
                .find(|q| q.namespace == p.namespace && q.name == p.name)
            {
                Some(existing) => existing.value = p.value,
                None => self.properties.push(p),
            }
        }
    }
}

/// XMP properties from the file's top-level XMP `uuid` box(es).
///
/// * `Ok(None)`: a valid file with no XMP box (also raw codestreams, which have no boxes).
/// * `Err`: an XMP box is present but nothing could be read from it.
///
/// Several XMP boxes are merged in file order, later values winning.
pub fn read_xmp(file: &[u8]) -> Result<Option<XmpMetadata>, String> {
    if file.starts_with(&[0xFF, 0x4F]) {
        return Ok(None); // raw codestream: no boxes
    }
    let mut merged: Option<XmpMetadata> = None;
    let mut first_error: Option<String> = None;
    for (kind, ps, pe) in boxes(file, 0, file.len()) {
        if &kind != b"uuid" || pe < ps + 16 || file[ps..ps + 16] != XMP_UUID {
            continue; // not XMP (other uuid boxes, e.g. georeferencing, are ignored)
        }
        let payload = &file[ps + 16..pe];
        if payload.len() > MAX_PACKET {
            first_error.get_or_insert(format!(
                "XMP packet is {} bytes, over the {} MiB limit",
                payload.len(),
                MAX_PACKET >> 20
            ));
            continue;
        }
        match parse_packet(payload) {
            Ok(props) => merged.get_or_insert_with(XmpMetadata::default).merge(props),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    match (merged, first_error) {
        (Some(m), _) => Ok(Some(m)),
        (None, Some(e)) => Err(e),
        (None, None) => Ok(None),
    }
}

/// [`read_xmp`] on a file, memory-mapped so a huge image is not read into memory.
pub fn read_xmp_file(path: &Path) -> Result<Option<XmpMetadata>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: read-only map; a concurrent change to the file can only produce odd text or an
    // error from the bounds-checked reader.
    let map = unsafe { Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;
    read_xmp(&map)
}

/// Parse one XMP packet into its simple properties.
fn parse_packet(bytes: &[u8]) -> Result<Vec<XmpProperty>, String> {
    // Packets are padded with whitespace (some writers add NULs) after the closing instruction.
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == 0 || bytes[end - 1].is_ascii_whitespace()) {
        end -= 1;
    }
    let mut body = &bytes[..end];
    if let Some(rest) = body.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        body = rest;
    }
    let text =
        std::str::from_utf8(body).map_err(|_| "XMP packet is not valid UTF-8".to_string())?;
    // No DTDs: they are how entity-expansion attacks are made, and XMP never needs one.
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") {
        return Err("XMP packet contains a DTD, which is not allowed".into());
    }
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: 1_000_000,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(text, options)
        .map_err(|e| format!("XMP is not valid XML: {e}"))?;
    let mut out = Vec::new();
    for node in doc.descendants() {
        if node.is_element() && is_rdf(node, "Description") {
            collect_description(node, &mut out);
        }
    }
    Ok(out)
}

fn is_rdf(node: Node, local: &str) -> bool {
    node.tag_name().namespace() == Some(RDF_NS) && node.tag_name().name() == local
}

fn property(node: Node, namespace: &str, name: &str, value: String) -> XmpProperty {
    XmpProperty {
        namespace: namespace.to_string(),
        prefix: node.lookup_prefix(namespace).unwrap_or("").to_string(),
        name: name.to_string(),
        value,
    }
}

/// The properties of one `rdf:Description`: attribute form first, then element form (the order
/// they appear in the text).
fn collect_description(desc: Node, out: &mut Vec<XmpProperty>) {
    for a in desc.attributes() {
        let Some(ns) = a.namespace() else { continue };
        if matches!(ns, RDF_NS | XML_NS | XMLNS_NS | META_NS) {
            continue;
        }
        out.push(property(desc, ns, a.name(), a.value().to_string()));
    }
    for child in desc.children().filter(|n| n.is_element()) {
        let Some(ns) = child.tag_name().namespace() else {
            continue;
        };
        if ns == RDF_NS {
            continue;
        }
        if let Some(value) = element_value(child) {
            out.push(property(child, ns, child.tag_name().name(), value));
        }
    }
}

fn text_of(node: Node) -> String {
    node.children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

/// Value of a property element: its text, or the `rdf:li` items of an `rdf:Seq`/`Bag`/`Alt` joined
/// with ", ". Structured values (anything more complex) are skipped, not errors.
fn element_value(child: Node) -> Option<String> {
    let elements: Vec<Node> = child.children().filter(|n| n.is_element()).collect();
    if elements.is_empty() {
        return Some(text_of(child));
    }
    let container = elements.first().filter(|_| elements.len() == 1)?;
    if !["Seq", "Bag", "Alt"].iter().any(|k| is_rdf(*container, k)) {
        return None;
    }
    let mut items = Vec::new();
    for li in container.children().filter(|n| n.is_element()) {
        if !is_rdf(li, "li") || li.children().any(|n| n.is_element()) {
            return None;
        }
        items.push(text_of(li));
    }
    Some(items.join(", "))
}

/// Break `text` into pieces of at most `width` characters, at spaces where possible.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while chars.len() - start > width {
        let window = &chars[start..start + width + 1];
        // Prefer the last space inside the window (but not at its very start).
        let cut = window
            .iter()
            .rposition(|&c| c == ' ')
            .filter(|&i| i > 0)
            .unwrap_or(width);
        out.push(chars[start..start + cut].iter().collect::<String>());
        start += cut;
        while start < chars.len() && chars[start] == ' ' && cut != width {
            start += 1; // drop the space we broke at
        }
    }
    out.push(chars[start..].iter().collect());
    out
}

/// What the Metadata panel and `tools meta` print for one file: a title line, then the properties
/// grouped by namespace (in order of first appearance), keys padded to a common column, long values
/// wrapped under the value column.
///
/// `raw_codestream` says the file has no JP2 boxes at all (it only changes the "no metadata" text).
pub fn describe_xmp(
    name: &str,
    state: &Result<Option<XmpMetadata>, String>,
    raw_codestream: bool,
    columns: usize,
) -> Vec<String> {
    let mut lines = vec![format!("Metadata  {name}")];
    let meta = match state {
        Err(e) => {
            lines.push(format!("Could not read embedded metadata: {e}"));
            return lines;
        }
        Ok(None) => {
            lines.push(if raw_codestream {
                "No embedded metadata (raw codestream, no JP2 boxes).".to_string()
            } else {
                "No embedded metadata (no XMP box in this file).".to_string()
            });
            return lines;
        }
        Ok(Some(m)) if m.properties.is_empty() => {
            lines.push("Embedded XMP has no simple properties.".to_string());
            return lines;
        }
        Ok(Some(m)) => m,
    };

    // Namespaces in order of first appearance.
    let mut groups: Vec<(&str, &str)> = Vec::new();
    for p in &meta.properties {
        if !groups.iter().any(|(ns, _)| *ns == p.namespace) {
            groups.push((&p.namespace, &p.prefix));
        }
    }
    let key_width = meta
        .properties
        .iter()
        .map(|p| p.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(MAX_KEY_COLUMN);
    let value_col = 2 + key_width + 2;
    let value_width = columns.saturating_sub(value_col).max(16);
    let indent = " ".repeat(value_col);

    for (ns, prefix) in groups {
        lines.push(format!(
            "XMP ({})",
            if prefix.is_empty() { ns } else { prefix }
        ));
        for p in meta.properties.iter().filter(|p| p.namespace == ns) {
            // Tabs and other control characters would break the column layout.
            let value: String = p
                .value
                .chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .collect();
            let mut pieces: Vec<String> = Vec::new();
            for line in value.split("\r\n").flat_map(|l| l.split(['\n', '\r'])) {
                pieces.extend(wrap(line, value_width));
            }
            let mut pieces = pieces.into_iter();
            if p.name.chars().count() > MAX_KEY_COLUMN {
                // Too wide for the key column: the key gets its own line.
                lines.push(format!("  {}", p.name));
                for piece in pieces {
                    lines.push(format!("{indent}{piece}").trim_end().to_string());
                }
            } else {
                let first = pieces.next().unwrap_or_default();
                lines.push(
                    format!("  {:<key_width$}  {first}", p.name)
                        .trim_end()
                        .to_string(),
                );
                for piece in pieces {
                    lines.push(format!("{indent}{piece}").trim_end().to_string());
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = "http://example.com/ns/layer/1.0/";

    /// A complete XMP packet around `body` (the content of one `rdf:Description`'s element list).
    fn packet(body: &str) -> String {
        format!(
            "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
             <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
             <rdf:Description rdf:about=\"\" xmlns:lay=\"{NS}\">\n{body}\n  </rdf:Description>\n \
             </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
        )
    }

    fn uuid_box(uuid: [u8; 16], payload: &[u8]) -> Vec<u8> {
        let mut v = ((8 + 16 + payload.len()) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(b"uuid");
        v.extend_from_slice(&uuid);
        v.extend_from_slice(payload);
        v
    }

    /// A JP2 file (dummy codestream) followed by `tail`.
    fn jp2_with(tail: &[u8]) -> Vec<u8> {
        let mut v = crate::wrap_jp2(&[0xFF, 0x4F, 1, 2, 3, 4], 10, 10);
        v.extend_from_slice(tail);
        v
    }

    fn xmp(file: &[u8]) -> XmpMetadata {
        read_xmp(file).expect("readable").expect("has XMP")
    }

    fn names(m: &XmpMetadata) -> Vec<(&str, &str)> {
        m.properties
            .iter()
            .map(|p| (p.name.as_str(), p.value.as_str()))
            .collect()
    }

    #[test]
    fn element_form_in_document_order_with_unescaping() {
        let body = "   <lay:Part_Name>A &amp; &lt;B&gt;</lay:Part_Name>\n\
                    <lay:Layer_Number>7</lay:Layer_Number>\n\
                    <lay:Material>ASA</lay:Material>";
        let m = xmp(&jp2_with(&uuid_box(XMP_UUID, packet(body).as_bytes())));
        assert_eq!(
            names(&m),
            vec![
                ("Part_Name", "A & <B>"),
                ("Layer_Number", "7"),
                ("Material", "ASA")
            ]
        );
        assert!(m
            .properties
            .iter()
            .all(|p| p.namespace == NS && p.prefix == "lay"));
    }

    #[test]
    fn attribute_form_is_supported() {
        let pk = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"tool\">\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:lay=\"{NS}\" lay:Layer_Number=\"4\"/>\
             </rdf:RDF></x:xmpmeta>"
        );
        let m = xmp(&jp2_with(&uuid_box(XMP_UUID, pk.as_bytes())));
        assert_eq!(
            names(&m),
            vec![("Layer_Number", "4")],
            "rdf:about and x:xmptk are not properties"
        );
    }

    #[test]
    fn no_xmp_box_is_ok_none() {
        assert_eq!(read_xmp(&jp2_with(&[])), Ok(None));
    }

    #[test]
    fn raw_codestream_is_ok_none() {
        assert_eq!(read_xmp(&[0xFF, 0x4F, 0xFF, 0x51, 0, 0, 0, 0]), Ok(None));
    }

    #[test]
    fn other_uuid_boxes_are_ignored() {
        let other = [7u8; 16];
        assert_eq!(
            read_xmp(&jp2_with(&uuid_box(other, packet("").as_bytes()))),
            Ok(None)
        );
    }

    #[test]
    fn xmp_box_with_lbox_zero_runs_to_end_of_file() {
        let pk = packet("<lay:A>1</lay:A>");
        let mut b = vec![0, 0, 0, 0];
        b.extend_from_slice(b"uuid");
        b.extend_from_slice(&XMP_UUID);
        b.extend_from_slice(pk.as_bytes());
        assert_eq!(names(&xmp(&jp2_with(&b))), vec![("A", "1")]);
    }

    #[test]
    fn jp2c_with_xlbox_is_walked_over() {
        // Rebuild the file with an extended-length (LBox = 1) codestream box before the XMP box.
        let cs = [0xFF, 0x4F, 9, 9, 9];
        let full = crate::wrap_jp2(&cs, 10, 10);
        let jp2c_at = full.len() - (8 + cs.len());
        let mut v = full[..jp2c_at].to_vec();
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(b"jp2c");
        v.extend_from_slice(&((16 + cs.len()) as u64).to_be_bytes());
        v.extend_from_slice(&cs);
        v.extend_from_slice(&uuid_box(XMP_UUID, packet("<lay:A>x</lay:A>").as_bytes()));
        assert_eq!(names(&xmp(&v)), vec![("A", "x")]);
    }

    #[test]
    fn dtd_and_entities_are_rejected_not_expanded() {
        let pk = "<!DOCTYPE x [<!ENTITY a \"b\">]><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
        let err = read_xmp(&jp2_with(&uuid_box(XMP_UUID, pk.as_bytes()))).unwrap_err();
        assert!(err.contains("DTD"), "{err}");
    }

    #[test]
    fn malformed_xml_is_an_error() {
        let r = read_xmp(&jp2_with(&uuid_box(XMP_UUID, b"<not xml")));
        assert!(r.unwrap_err().contains("not valid XML"));
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        let r = read_xmp(&jp2_with(&uuid_box(XMP_UUID, &[b'<', 0xFF, 0xFE, b'>'])));
        assert!(r.unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn numeric_character_reference_for_carriage_return() {
        let m = xmp(&jp2_with(&uuid_box(
            XMP_UUID,
            packet("<lay:Note>a&#13;b</lay:Note>").as_bytes(),
        )));
        assert_eq!(m.properties[0].value, "a\rb");
    }

    #[test]
    fn sequences_and_bags_are_joined() {
        let body = "<lay:Tags><rdf:Seq><rdf:li>a</rdf:li><rdf:li>b</rdf:li></rdf:Seq></lay:Tags>\n\
                    <lay:Set><rdf:Bag><rdf:li>x</rdf:li></rdf:Bag></lay:Set>";
        let m = xmp(&jp2_with(&uuid_box(XMP_UUID, packet(body).as_bytes())));
        assert_eq!(names(&m), vec![("Tags", "a, b"), ("Set", "x")]);
    }

    #[test]
    fn structured_values_are_skipped_not_fatal() {
        let body = "<lay:Nested><lay:Inner>1</lay:Inner></lay:Nested>\n<lay:Plain>ok</lay:Plain>";
        let m = xmp(&jp2_with(&uuid_box(XMP_UUID, packet(body).as_bytes())));
        assert_eq!(names(&m), vec![("Plain", "ok")]);
    }

    #[test]
    fn truncated_files_never_panic() {
        let file = jp2_with(&uuid_box(
            XMP_UUID,
            packet("<lay:A>1</lay:A><lay:B>2</lay:B>").as_bytes(),
        ));
        for n in 0..=file.len() {
            let _ = read_xmp(&file[..n]);
        }
        // Also when only the box header and UUID are present.
        let _ = read_xmp(&file[..file.len() - 30]);
    }

    #[test]
    fn oversized_packet_is_an_error_not_a_parse() {
        let payload = vec![b' '; MAX_PACKET + 1];
        let r = read_xmp(&jp2_with(&uuid_box(XMP_UUID, &payload)));
        assert!(r.unwrap_err().contains("limit"));
    }

    #[test]
    fn several_xmp_boxes_merge_later_value_wins() {
        let a = packet("<lay:A>first</lay:A><lay:B>keep</lay:B>");
        let b = packet("<lay:A>second</lay:A><lay:C>new</lay:C>");
        let mut tail = uuid_box(XMP_UUID, a.as_bytes());
        tail.extend_from_slice(&uuid_box(XMP_UUID, b.as_bytes()));
        let m = xmp(&jp2_with(&tail));
        assert_eq!(
            names(&m),
            vec![("A", "second"), ("B", "keep"), ("C", "new")]
        );
    }

    #[test]
    fn any_namespace_is_reported_with_its_prefix() {
        let pk = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                  <rdf:Description xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
                  <dc:format>image/jp2</dc:format></rdf:Description></rdf:RDF></x:xmpmeta>";
        let m = xmp(&jp2_with(&uuid_box(XMP_UUID, pk.as_bytes())));
        let p = &m.properties[0];
        assert_eq!(
            (
                p.namespace.as_str(),
                p.prefix.as_str(),
                p.name.as_str(),
                p.value.as_str()
            ),
            (
                "http://purl.org/dc/elements/1.1/",
                "dc",
                "format",
                "image/jp2"
            )
        );
    }

    #[test]
    fn padding_after_the_packet_and_a_leading_bom_are_tolerated() {
        let mut payload = vec![0xEF, 0xBB, 0xBF];
        payload.extend_from_slice(packet("<lay:A>1</lay:A>").as_bytes());
        payload.extend_from_slice(&[b' '; 200]);
        payload.extend_from_slice(&[0, 0, 0]);
        assert_eq!(
            names(&xmp(&jp2_with(&uuid_box(XMP_UUID, &payload)))),
            vec![("A", "1")]
        );
    }

    #[test]
    fn empty_element_has_an_empty_value() {
        let m = xmp(&jp2_with(&uuid_box(
            XMP_UUID,
            packet("<lay:Empty/><lay:Gap></lay:Gap>").as_bytes(),
        )));
        assert_eq!(names(&m), vec![("Empty", ""), ("Gap", "")]);
    }

    fn meta(props: &[(&str, &str, &str, &str)]) -> XmpMetadata {
        XmpMetadata {
            properties: props
                .iter()
                .map(|(ns, prefix, name, value)| XmpProperty {
                    namespace: ns.to_string(),
                    prefix: prefix.to_string(),
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn describe_groups_pads_and_keeps_order() {
        let m = meta(&[
            (NS, "lay", "Part_Name", "Bracket"),
            (NS, "lay", "Layer_Number", "7"),
            (
                "http://purl.org/dc/elements/1.1/",
                "dc",
                "format",
                "image/jp2",
            ),
            (NS, "lay", "Material", "ASA"),
        ]);
        let lines = describe_xmp("layer_7.jp2", &Ok(Some(m)), false, XMP_COLUMNS);
        assert_eq!(
            lines,
            vec![
                "Metadata  layer_7.jp2",
                "XMP (lay)",
                "  Part_Name     Bracket",
                "  Layer_Number  7",
                "  Material      ASA",
                "XMP (dc)",
                "  format        image/jp2",
            ]
        );
    }

    #[test]
    fn describe_wraps_long_values_under_the_value_column() {
        let long = "word ".repeat(30).trim_end().to_string(); // 149 chars
        let m = meta(&[(NS, "lay", "Note", &long), (NS, "lay", "Id", "x")]);
        let lines = describe_xmp("f.jp2", &Ok(Some(m)), false, 40);
        assert!(lines.len() > 5, "{lines:#?}");
        let indent = " ".repeat(2 + 4 + 2);
        for l in &lines[3..lines.len() - 1] {
            assert!(l.starts_with(&indent), "continuation not indented: {l:?}");
            assert!(l.chars().count() <= 40, "too wide: {l:?}");
        }
        assert!(lines[2].starts_with("  Note  word"));
        // Nothing was lost or invented by wrapping.
        let rejoined: Vec<&str> = lines[2..lines.len() - 1]
            .iter()
            .flat_map(|l| l.split_whitespace())
            .filter(|w| *w != "Note")
            .collect();
        assert_eq!(rejoined.len(), 30);
    }

    #[test]
    fn describe_hard_wraps_a_value_without_spaces() {
        let uuid = "3f2c9a1e-5b7d-4c2a-9e8f-0123456789ab-0123456789abcdef0123456789abcdef";
        let m = meta(&[(NS, "lay", "CMB_UUID", uuid)]);
        let lines = describe_xmp("f.jp2", &Ok(Some(m)), false, 40);
        let all: String = lines[2..]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("");
        assert!(all.ends_with(uuid) || all.contains(uuid), "{lines:#?}");
        assert!(
            lines.iter().skip(2).all(|l| l.chars().count() <= 40),
            "{lines:#?}"
        );
    }

    #[test]
    fn describe_splits_multi_line_values() {
        let m = meta(&[(NS, "lay", "Note", "line one\r\nline two\nline three")]);
        let lines = describe_xmp("f.jp2", &Ok(Some(m)), false, XMP_COLUMNS);
        assert_eq!(
            &lines[2..],
            ["  Note  line one", "        line two", "        line three"]
        );
    }

    #[test]
    fn describe_puts_over_wide_keys_on_their_own_line() {
        let key = "K".repeat(40);
        let m = meta(&[(NS, "lay", &key, "v"), (NS, "lay", "Short", "s")]);
        let lines = describe_xmp("f.jp2", &Ok(Some(m)), false, XMP_COLUMNS);
        assert_eq!(lines[2], format!("  {key}"));
        assert_eq!(lines[3].trim(), "v");
        assert!(
            lines[4].starts_with(&format!("  {:<32}  s", "Short")),
            "{:?}",
            lines[4]
        );
    }

    #[test]
    fn describe_states() {
        let none = describe_xmp("a.jp2", &Ok(None), false, XMP_COLUMNS);
        assert_eq!(
            none,
            [
                "Metadata  a.jp2",
                "No embedded metadata (no XMP box in this file)."
            ]
        );
        let raw = describe_xmp("a.j2c", &Ok(None), true, XMP_COLUMNS);
        assert_eq!(
            raw[1],
            "No embedded metadata (raw codestream, no JP2 boxes)."
        );
        let err = describe_xmp("a.jp2", &Err("boom".into()), false, XMP_COLUMNS);
        assert_eq!(err[1], "Could not read embedded metadata: boom");
        let empty = describe_xmp(
            "a.jp2",
            &Ok(Some(XmpMetadata::default())),
            false,
            XMP_COLUMNS,
        );
        assert_eq!(empty[1], "Embedded XMP has no simple properties.");
    }

    #[test]
    fn describe_a_real_packet_end_to_end() {
        let body = "<lay:Part_Name>Bracket</lay:Part_Name><lay:Layer_Number>7</lay:Layer_Number>";
        let file = jp2_with(&uuid_box(XMP_UUID, packet(body).as_bytes()));
        let lines = describe_xmp("layer_7.jp2", &read_xmp(&file), false, XMP_COLUMNS);
        assert_eq!(lines[0], "Metadata  layer_7.jp2");
        assert!(
            lines.contains(&"  Part_Name     Bracket".to_string()),
            "{lines:#?}"
        );
    }
}
