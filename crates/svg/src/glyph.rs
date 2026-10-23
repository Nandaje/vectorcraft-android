//! OpenType SVG glyphs: the art of a font's glyph from the SVG document its `SVG ` table holds for
//! it, made as the importer makes art of any SVG file. The text crate asks for it through the
//! reader the app registers ([`vectorcraft_text::set_svg_glyph_reader`]).
//!
//! Where it differs from the `SVG ` table's rules: a size on the root (`width`, `height`) counts
//! for nothing; a non-square `viewBox` is scaled to fit the em by its larger side, not centred in
//! it; and a glyph element nested in others keeps what they apply (transforms, opacity).

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use usvg::roxmltree;
use vectorcraft_doc::color::Color;
use vectorcraft_doc::{Node, NodeId};
use vectorcraft_geom::Affine;

use crate::ImportOptions;

/// The most bytes a glyph's document may have, unpacked.
const MAX_SVG_GLYPH_BYTES: u64 = 1 << 20;

/// Elements left out of a glyph: text (type inside type), images (a glyph's art has no document
/// to keep them in), embedded content and scripts. The `SVG ` table rules text, embedded content
/// and scripts out, and allows images.
const LEFT_OUT: [&str; 4] = ["text", "image", "foreignObject", "script"];

/// Glyph `gid`'s art from its SVG `document` (possibly gzipped), in a font of `upem` units per em:
/// its element `glyph<gid>` (or the whole document when that is its root), in font units (y down),
/// a `viewBox` mapped onto the em square. The text's colour is `fill`: `currentColor`, and the
/// legacy `context-fill` and `context-stroke` (none without a `stroke`). Its nodes have no ids
/// (they aren't a document's). With the art, whether it uses the text's colours. `None` when the
/// document doesn't read, is too large or has no such element.
pub fn glyph_art(document: &[u8], gid: u32, upem: f64, fill: Color, stroke: Option<Color>) -> Option<(Node, bool)> {
    // The SVGZ budget left is the glyph's cap.
    let unpacked = AtomicU64::new(crate::MAX_SVGZ.saturating_sub(MAX_SVG_GLYPH_BYTES));
    let text = crate::text_within(document, &unpacked).ok()?;
    if text.len() as u64 > MAX_SVG_GLYPH_BYTES {
        return None;
    }
    let id = format!("glyph{gid}");
    let hex = |c: Color| {
        let [r, g, b, _] = c.to_rgba8(1.0);
        format!("#{r:02x}{g:02x}{b:02x}")
    };
    let Glyph { svg, whole, view_box } = glyph(&text, &id, &hex(fill))?;
    let coloured = ["currentColor", "currentcolor", "context-fill", "context-stroke"].iter().any(|k| svg.contains(k));
    // usvg resolves `context-fill` and `context-stroke` only for markers.
    let svg = svg.replace("context-fill", &hex(fill)).replace("context-stroke", &stroke.map_or_else(|| "none".into(), hex));
    let (doc, _) = crate::import::import(&svg, &ImportOptions::default()).ok()?;
    let mut art = if whole {
        Node::group(NodeId(0), doc.layers.iter().map(|l| Arc::new(Node::group(NodeId(0), l.children().cloned().unwrap_or_default()))).collect())
    } else {
        // A top-level element is a layer, others an object, named after its id.
        let named = |n: &Node| n.name.as_deref() == Some(id.as_str());
        match doc.layers.iter().find(|l| named(l)) {
            Some(layer) => Node::group(NodeId(0), layer.children().cloned().unwrap_or_default()),
            None => {
                let mut found = None;
                for layer in &doc.layers {
                    layer.walk(&mut |n| {
                        if found.is_none() && named(n) {
                            found = Some(n.clone());
                        }
                    });
                }
                found?
            }
        }
    };
    // Without a size of its own, the document's viewport is its view box at one unit per unit:
    // the em square is that box.
    if let Some(side) = view_box.filter(|s| *s > 0.0 && s.is_finite()) {
        art.transform(Affine::scale(upem / side), true);
    }
    Some((anonymous(&art), coloured))
}

/// A glyph's document made ready for the importer.
struct Glyph {
    svg: String,
    /// The glyph is the whole document (its root has the glyph's id).
    whole: bool,
    /// The larger side of the root's `viewBox`, if it has one.
    view_box: Option<f64>,
}

/// `svg` with only what glyph `id` draws: without the other glyphs' top-level elements (a font
/// may keep all its glyphs in one document), the [`LEFT_OUT`] elements and event attributes, the
/// root without a size (the em square is its view box) and in the text's colour `color`. `None`
/// when it doesn't read or has no element `id`.
fn glyph(svg: &str, id: &str, color: &str) -> Option<Glyph> {
    let xml = roxmltree::Document::parse_with_options(svg, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).ok()?;
    let root = xml.root_element();
    let whole = root.attribute("id") == Some(id);
    if !whole && !xml.descendants().any(|n| n.attribute("id") == Some(id)) {
        return None;
    }
    let is_glyph = |v: &str| v.strip_prefix("glyph").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    // Edits: byte ranges to cut and what to put in their place, in order.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = vec![];
    for n in root.children().filter(|n| n.attribute("id").is_some_and(|v| v != id && is_glyph(v))) {
        edits.push((n.range(), String::new()));
    }
    for n in root.descendants().filter(|n| n.is_element() && LEFT_OUT.contains(&n.tag_name().name())) {
        edits.push((n.range(), String::new()));
    }
    for a in root.attributes().filter(|a| matches!(a.name(), "width" | "height" | "color")) {
        edits.push((a.range(), String::new()));
    }
    // A glyph is art: event attributes (scripts) go too.
    for a in root.descendants().filter(|n| n.is_element()).flat_map(|n| n.attributes()).filter(|a| a.name().starts_with("on")) {
        edits.push((a.range(), String::new()));
    }
    // The text's colour, right after the root's name.
    let start = root.range().start + 1;
    let name = start + svg.get(start..)?.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    edits.push((name..name, format!(" color=\"{color}\"")));
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    for (r, with) in edits {
        // An element inside one already cut goes with it.
        if r.start < at {
            continue;
        }
        out.push_str(svg.get(at..r.start)?);
        out.push_str(&with);
        at = r.end;
    }
    out.push_str(svg.get(at..)?);
    let view_box = root.attribute("viewBox").and_then(|v| {
        let n: Vec<f64> = v.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        match n.as_slice() {
            [_, _, w, h] => Some(w.max(*h)),
            _ => None,
        }
    });
    Some(Glyph { svg: out, whole, view_box })
}

/// `n` with no ids, its members' neither.
fn anonymous(n: &Node) -> Node {
    let mut n = n.clone();
    n.id = NodeId(u64::MAX);
    if let Some(children) = n.children_mut() {
        for c in children.iter_mut() {
            *c = Arc::new(anonymous(c));
        }
    }
    n
}
