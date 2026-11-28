//! Formula images.
//!
//! `lmdv-md` replaces every formula with a U+FFFC placeholder and records its
//! TeX; the renderer asks a [`MathResolver`] for a picture, fits its box to the
//! reading column and hands Rune an [`InlineImage`] attached with
//! [`rune::El::inline_image`]. A resolver that returns `None` leaves the TeX
//! visible in a code chip, so a failed formula never drops the paragraph or the
//! block (M-34).
//!
//! Rune keeps an `InlineImage`'s metrics private, so the resolver returns a
//! [`MathImage`] carrying them: this module does the fitting and the accessible
//! label, then Rune puts the image on the baseline from those numbers.

use rune::InlineImage;

/// A rendered formula with the point metrics Rune needs to place it on a line.
///
/// Rune's [`InlineImage`] owns its pixels and metrics privately; carrying them
/// here lets the renderer scale the box to the column and speak the TeX through
/// the image's accessible label (upstream substitutes that label into the
/// text's accessible value while selection, copy and find still see U+FFFC).
#[derive(Clone, Debug)]
pub struct MathImage {
    /// The decoded pixels drawn for the formula.
    pub pixels: rune::Pixels,
    /// The drawn width in points.
    pub width: f32,
    /// The room above the baseline in points.
    pub ascent: f32,
    /// The room below the baseline in points.
    pub descent: f32,
}

impl MathImage {
    /// An image `width` points wide, sitting `ascent` above and `descent` below
    /// the baseline. Fractional points are kept; Rune no longer rounds them to
    /// whole device pixels.
    pub fn new(pixels: rune::Pixels, width: f32, ascent: f32, descent: f32) -> Self {
        Self { pixels, width: width.max(0.0), ascent: ascent.max(0.0), descent: descent.max(0.0) }
    }

    /// Scales the box down until its height (`ascent` plus `descent`) is at most
    /// `max_height` and its width at most `max_width`, keeping the aspect ratio
    /// and the baseline split. Non-positive bounds are ignored; an image already
    /// inside both keeps its exact metrics.
    pub fn fit(mut self, max_width: f32, max_height: f32) -> Self {
        let height = (self.ascent + self.descent).max(0.001);
        let width = self.width.max(0.001);
        let mut scale = 1.0f32;
        if max_height > 0.0 {
            scale = scale.min(max_height / height);
        }
        if max_width > 0.0 && width * scale > max_width {
            scale = max_width / width;
        }
        if scale < 1.0 {
            self.width = width * scale;
            self.ascent *= scale;
            self.descent *= scale;
        }
        self
    }

    /// The Rune image, with `label` spoken in place of the replaced bytes.
    pub fn into_image(self, label: impl Into<String>) -> InlineImage {
        InlineImage::new(self.pixels, self.width, self.ascent, self.descent).label(label)
    }
}

/// Supplies rasterized formula images.
///
/// The app wraps `lmdv-math`'s `MathRenderer`: `image` is the window's image
/// cache entry, and the three point sizes are the layout box at the requested
/// body size. The renderer scales what it gets to fit the reading column, so a
/// resolver can work at one size.
pub trait MathResolver {
    /// An inline formula at `size_pt` body size.
    fn inline(&self, tex: &str, size_pt: f32) -> Option<MathImage>;
    /// A display formula at `size_pt` body size.
    fn display(&self, tex: &str, size_pt: f32) -> Option<MathImage>;
}

/// No formula pictures: the TeX source shows in a code chip.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMath;

impl MathResolver for NoMath {
    fn inline(&self, _tex: &str, _size_pt: f32) -> Option<MathImage> {
        None
    }

    fn display(&self, _tex: &str, _size_pt: f32) -> Option<MathImage> {
        None
    }
}

#[cfg(test)]
mod tests {
    use rune::Pixels;

    use super::*;

    fn image(width: f32, ascent: f32, descent: f32) -> MathImage {
        let pixels = Pixels::new();
        pixels.set(1, 1, vec![255, 0, 0, 255]);
        MathImage::new(pixels, width, ascent, descent)
    }

    #[test]
    fn fitting_keeps_the_aspect_ratio() {
        let fitted = image(100.0, 30.0, 20.0).fit(50.0, 100.0);
        assert!((fitted.width - 50.0).abs() < 0.01);
        assert!((fitted.ascent - 15.0).abs() < 0.01);
        assert!((fitted.descent - 10.0).abs() < 0.01);
        assert!((fitted.ascent + fitted.descent - 25.0).abs() < 0.01);
    }

    #[test]
    fn fitting_leaves_small_images_alone() {
        let original = image(20.0, 6.0, 4.0);
        let fitted = original.clone().fit(100.0, 40.0);
        assert_eq!(fitted.width, original.width);
        assert_eq!(fitted.ascent, original.ascent);
        assert_eq!(fitted.descent, original.descent);
    }

    #[test]
    fn fitting_a_display_formula_uses_the_combined_height() {
        let fitted = image(100.0, 30.0, 20.0).fit(200.0, 25.0);
        assert!((fitted.width - 50.0).abs() < 0.01, "the 50 point height is the limit");
        assert!((fitted.ascent + fitted.descent - 25.0).abs() < 0.01);
    }
}
