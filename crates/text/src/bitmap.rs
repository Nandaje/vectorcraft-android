//! Colour bitmap glyphs (`sbix`, as in Apple Color Emoji, and `CBDT`/`CBLC`, as in Noto Color
//! Emoji): the PNG of a glyph's largest strike, placed in font units. Other graphic types (`sbix`
//! `dupe`, `flip`, `jpg `, `tiff`; `CBDT` BGRA and mask data) and `EBDT` (hinting bitmaps of
//! outline fonts) draw nothing.

use kurbo::{Affine, Rect};
use skrifa::GlyphId;
use skrifa::bitmap::{BitmapData, BitmapFormat, BitmapStrikes, Origin};
use skrifa::instance::Size;
use vectorcraft_doc::ImageBlob;

/// The most pixels along either side of a glyph image read from a font, as its PNG header gives
/// them. Emoji strikes are at most a few hundred pixels (Apple Color Emoji's largest is 160): a
/// font is untrusted input, and each glyph image is decoded and kept apart (at most 4 MB of pixels).
pub(crate) const MAX_SIDE: u32 = 1024;

/// The most bytes of a glyph image's PNG: twice its largest pixels' raw size, which a PNG
/// (compressed, or stored uncompressed) stays well within.
pub(crate) const MAX_PNG_BYTES: usize = 2 * 4 * (MAX_SIDE as usize) * (MAX_SIDE as usize);

/// A colour bitmap glyph: a PNG drawn in place of an outline. Fill and stroke don't paint it.
#[derive(Debug)]
pub struct GlyphImage {
    /// The PNG, as image objects hold their pixels.
    pub blob: ImageBlob,
    pub width: u32,
    pub height: u32,
    /// Pixel space (0..width, 0..height, y down) → font units (y down, as outlines are).
    pub xf: Affine,
}

impl GlyphImage {
    /// The image's pixel rectangle (0..width, 0..height).
    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width as f64, self.height as f64)
    }
}

/// Does `font` have colour bitmap strikes?
pub(crate) fn has_colour_bitmaps(font: &skrifa::FontRef<'_>) -> bool {
    strikes(font).is_some_and(|s| !s.is_empty())
}

fn strikes<'a>(font: &skrifa::FontRef<'a>) -> Option<BitmapStrikes<'a>> {
    BitmapStrikes::with_format(font, BitmapFormat::Sbix).or_else(|| BitmapStrikes::with_format(font, BitmapFormat::Cbdt))
}

/// Glyph `gid`'s image in its largest strike, if it is a PNG.
pub(crate) fn glyph_image(font: &skrifa::FontRef<'_>, upem: f64, gid: u32) -> Option<GlyphImage> {
    let strikes = strikes(font)?;
    let sbix = strikes.format() == Some(BitmapFormat::Sbix);
    let g = strikes.glyph_for_size(Size::unscaled(), GlyphId::new(gid))?;
    let BitmapData::Png(png) = g.data else { return None };
    // The size the PNG decodes to is its header's (`CBDT`'s own metrics may say otherwise).
    let (w, h) = png_size(png)?;
    let (ppem_x, ppem_y) = (f64::from(g.ppem_x), f64::from(g.ppem_y));
    if png.len() > MAX_PNG_BYTES || w > MAX_SIDE || h > MAX_SIDE || !(ppem_x >= 1.0 && ppem_y >= 1.0 && ppem_x.is_finite() && ppem_y.is_finite()) {
        return None;
    }
    let (sx, sy) = (upem / ppem_x, upem / ppem_y);
    // The image's top left in font units (y up): the outer bearings in font units, the inner ones
    // in pixels, from the bottom left corner for `sbix`.
    let left = f64::from(g.bearing_x) + f64::from(g.inner_bearing_x) * sx;
    let mut top = f64::from(g.bearing_y) + f64::from(g.inner_bearing_y) * sy;
    if g.placement_origin == Origin::BottomLeft {
        top += h as f64 * sy;
    }
    // Apple Color Emoji sits lower than its `sbix` table says (CoreText's look, which the PDF
    // writer's Type 3 glyphs imitate too).
    if sbix {
        top -= 0.128 * upem;
    }
    if !(left.is_finite() && top.is_finite()) {
        return None;
    }
    Some(GlyphImage { blob: ImageBlob::png(png.to_vec()), width: w, height: h, xf: Affine::new([sx, 0.0, 0.0, sy, left, -top]) })
}

/// The width and height in PNG `bytes`' header (its signature, then the IHDR chunk); `None` when
/// it doesn't read or a side is 0.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let be = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let (w, h) = (be(16)?, be(20)?);
    (w > 0 && h > 0).then_some((w, h))
}
