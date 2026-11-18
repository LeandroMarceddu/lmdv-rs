//! Formula images.
//!
//! `lmdv-md` replaces every formula with a U+FFFC placeholder and records its
//! TeX; the renderer asks a [`MathResolver`] for a picture and draws it inline
//! with [`rune::Span::image`] (patch 0001 gives it independent width, height
//! and descent). A resolver that returns `None` leaves the TeX visible in a
//! code chip, so a failed formula never drops the paragraph or the block
//! (M-34).

use rune::text::InlineImage;

/// Supplies rasterized formula images.
///
/// The app wraps `lmdv-math`'s `MathImage`: `image` is the window's image cache
/// entry, and the three point sizes are the layout box at the requested body
/// size. The renderer scales what it gets to fit the reading column, so a
/// resolver can work at one size.
pub trait MathResolver {
    /// An inline formula at `size_pt` body size.
    fn inline(&self, tex: &str, size_pt: f32) -> Option<InlineImage>;
    /// A display formula at `size_pt` body size.
    fn display(&self, tex: &str, size_pt: f32) -> Option<InlineImage>;
}

/// No formula pictures: the TeX source shows in a code chip.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMath;

impl MathResolver for NoMath {
    fn inline(&self, _tex: &str, _size_pt: f32) -> Option<InlineImage> {
        None
    }

    fn display(&self, _tex: &str, _size_pt: f32) -> Option<InlineImage> {
        None
    }
}

/// Scales `image` down until its height is at most `max_height` and its width at
/// most `max_width`, keeping the baseline ratio.
pub(crate) fn fit(mut image: InlineImage, max_width: f32, max_height: f32) -> InlineImage {
    let height = image.height.max(0.001);
    let width = image.width.max(0.001);
    let mut scale = 1.0f32;
    if max_height > 0.0 {
        scale = scale.min(max_height / height);
    }
    if max_width > 0.0 && width * scale > max_width {
        scale = max_width / width;
    }
    if scale < 1.0 {
        image.width = width * scale;
        image.height = height * scale;
        image.ascent *= scale;
        image.descent *= scale;
    }
    image
}

#[cfg(test)]
mod tests {
    use rune::Pixels;
    use rune::text::InlineImage;

    use super::*;

    fn image(width: f32, height: f32) -> InlineImage {
        let pixels = Pixels::new();
        pixels.set(1, 1, vec![255, 0, 0, 255]);
        InlineImage::new(pixels, width, height).ascent(height * 0.6).descent(height * 0.2)
    }

    #[test]
    fn fitting_keeps_the_aspect_ratio() {
        let fitted = fit(image(100.0, 50.0), 50.0, 100.0);
        assert!((fitted.width - 50.0).abs() < 0.01);
        assert!((fitted.height - 25.0).abs() < 0.01);
        assert!((fitted.descent - 5.0).abs() < 0.01);
    }

    #[test]
    fn fitting_leaves_small_images_alone() {
        let original = image(20.0, 10.0);
        let fitted = fit(original.clone(), 100.0, 40.0);
        assert_eq!(fitted.width, original.width);
        assert_eq!(fitted.height, original.height);
    }
}
