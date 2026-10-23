//! OpenType SVG glyphs: `glyph_art` makes a glyph's art of its SVG document, as the importer opens
//! SVG files, in font units.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md › Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use vectorcraft_doc::color::{Color, Paint};
use vectorcraft_doc::{Node, NodeKind};
use vectorcraft_svg::glyph_art;

const BLUE: Color = Color::Rgb { r: 0.0, g: 0.0, b: 1.0 };

/// [`glyph_art`] in a font of 1000 units per em, in black text.
fn art(doc: &str, gid: u32) -> Option<Node> {
    glyph_art(doc.as_bytes(), gid, 1000.0, Color::BLACK, None).map(|(n, _)| n)
}

fn fills(n: &Node) -> Vec<Paint> {
    let mut out = vec![];
    n.walk(&mut |c| out.push(c.appearance.fill_paint()));
    out
}

#[test]
fn the_glyphs_element_is_its_art_in_font_units() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg">
      <defs><linearGradient id="g"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient></defs>
      <g id="glyph3"><path fill="#0f0" d="M0 0h10v-10z"/></g>
      <g id="glyph7"><path fill="url(#g)" d="M100 0h500v-700h-500z"/></g>
    </svg>"##;
    let art7 = art(doc, 7).unwrap();
    let b = art7.geometric_bounds().unwrap();
    assert!((b.x0 - 100.0).abs() < 1e-6 && (b.x1 - 600.0).abs() < 1e-6 && (b.y0 + 700.0).abs() < 1e-6 && b.y1.abs() < 1e-6, "{b:?}");
    let fills = fills(&art7);
    assert!(fills.iter().any(|p| matches!(p, Paint::Gradient(_))), "its gradient: {fills:?}");
    assert!(!fills.contains(&Paint::solid(Color::rgb8(0, 255, 0))), "not the other glyph's art");
    assert!(art(doc, 5).is_none(), "no element, no art");
}

#[test]
fn a_glyph_that_is_its_whole_document_is_its_art() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" id="glyph9"><path fill="#f00" d="M0 0h500v-500z"/></svg>"##;
    assert_eq!(fills(&art(doc, 9).unwrap()).iter().filter(|p| **p == Paint::solid(Color::rgb8(255, 0, 0))).count(), 1);
}

#[test]
fn current_color_and_context_colours_are_the_texts() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg"><g id="glyph1">
      <path fill="currentColor" d="M0 0h500v-500z"/>
      <path fill="context-fill" stroke="context-stroke" stroke-width="10" d="M0 0h500v-500z"/>
    </g></svg>"##;
    let (art, coloured) = glyph_art(doc.as_bytes(), 1, 1000.0, BLUE, Some(Color::rgb8(255, 0, 0))).unwrap();
    assert!(coloured);
    let mut seen = vec![];
    art.walk(&mut |c| seen.push((c.appearance.fill_paint(), c.appearance.stroke_paint())));
    assert!(seen.contains(&(Paint::solid(BLUE), Paint::None)), "currentColor: {seen:?}");
    assert!(seen.contains(&(Paint::solid(BLUE), Paint::solid(Color::rgb8(255, 0, 0)))), "context-fill and -stroke: {seen:?}");
    let (art, _) = glyph_art(doc.as_bytes(), 1, 1000.0, BLUE, None).unwrap();
    let mut strokes = vec![];
    art.walk(&mut |c| strokes.push(c.appearance.stroke_paint()));
    assert!(strokes.iter().all(|p| *p == Paint::None), "no stroke without the text's: {strokes:?}");
    let plain = r##"<svg xmlns="http://www.w3.org/2000/svg"><g id="glyph1"><path fill="#f00" d="M0 0h5v-5z"/></g></svg>"##;
    assert!(!glyph_art(plain.as_bytes(), 1, 1000.0, BLUE, None).unwrap().1, "a document in its own colours");
}

#[test]
fn a_view_box_is_the_em_square() {
    // The view box 2000 units wide is the em: in a font of 1000 units per em, art at 500 is at 250,
    // in one of 2048 at 512; a size on the root counts for nothing.
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10pt" height="10pt" viewBox="0 0 2000 2000">
      <g id="glyph2"><path fill="#000" d="M500 500h500v500h-500z"/></g></svg>"##;
    for (upem, x) in [(1000.0, 250.0), (2048.0, 512.0)] {
        let b = glyph_art(doc.as_bytes(), 2, upem, Color::BLACK, None).unwrap().0.geometric_bounds().unwrap();
        assert!((b.x0 - x).abs() < 1e-6 && (b.width() - x).abs() < 1e-6, "{upem}: {b:?}");
    }
}

/// Text (which would lay type out inside type), images, embedded content and scripts are left out
/// of a glyph, as the OpenType `SVG ` table asks.
#[test]
fn text_images_and_scripts_are_left_out() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><g id="glyph1">
      <path fill="#f00" d="M0 0h500v-500z"/>
      <text font-family="Svgtest Glyphs" font-size="500">E<tspan>F</tspan></text>
      <image width="10" height="10" xlink:href="data:image/png;base64,iVBORw0KGgo="/>
      <script>alert(1)</script>
    </g></svg>"##;
    let art = art(doc, 1).unwrap();
    let mut kinds = vec![];
    art.walk(&mut |n| kinds.push(n.kind_label()));
    assert!(art.children().is_some_and(|c| !c.is_empty()), "the path stays");
    let mut other = false;
    art.walk(&mut |n| other |= matches!(n.kind, NodeKind::Text(_) | NodeKind::Image(_)));
    assert!(!other, "{kinds:?}");
}

#[test]
fn gzipped_documents_are_read_and_malformed_ones_are_no_art() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg"><g id="glyph4"><path fill="#000" d="M0 0h10v-10z"/></g></svg>"##;
    assert!(glyph_art(&vectorcraft_svg::compress(doc), 4, 1000.0, Color::BLACK, None).is_some(), "gzipped");
    assert!(art("not xml <", 4).is_none());
    // A document that unpacks past the glyph cap (a bomb).
    let huge = format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><g id=\"glyph4\"><!--{}--></g></svg>", "x".repeat(4 << 20));
    assert!(glyph_art(&vectorcraft_svg::compress(&huge), 4, 1000.0, Color::BLACK, None).is_none());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A font's glyph documents are untrusted: whatever they hold, reading one doesn't panic.
    #[test]
    fn any_document_reads_without_panicking(body in "[<>/=\"a-z0-9 #:.()-]{0,200}", bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let doc = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><g id=\"glyph1\">{body}</g></svg>");
        let _ = glyph_art(doc.as_bytes(), 1, 1000.0, Color::BLACK, Some(BLUE));
        let _ = glyph_art(&bytes, 1, 0.0, Color::BLACK, None);
    }
}

/// A left-out element inside another glyph's element goes with it, and the glyph asked for, nested
/// in a group that holds other glyphs' art, is still found.
#[test]
fn cuts_inside_cuts_and_nested_glyphs_read() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg">
      <g id="glyph1"><path fill="#0f0" d="M0 0h5v-5z"/><text>A</text></g>
      <g><g id="glyph2"><path fill="#f00" d="M0 0h500v-500z"/><script>x</script></g></g>
    </svg>"##;
    assert!(fills(&art(doc, 2).unwrap()).contains(&Paint::solid(Color::rgb8(255, 0, 0))));
    assert!(art(doc, 1).is_some_and(|a| fills(&a).contains(&Paint::solid(Color::rgb8(0, 255, 0)))));
}

/// A font's glyph is art, never a script: event attributes on its shapes are left out, so no
/// export carries them on.
#[test]
fn event_attributes_are_left_out() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" onload="a()"><g id="glyph1" onclick="b()">
      <path id="p" fill="#f00" onmouseover="c()" d="M0 0h500v-500z"/></g></svg>"##;
    let art = art(doc, 1).unwrap();
    let mut events = vec![];
    art.walk(&mut |n| events.extend(n.extras().attrs.as_ref().map(|a| a.events.clone()).unwrap_or_default()));
    assert!(events.is_empty(), "{events:?}");
}
