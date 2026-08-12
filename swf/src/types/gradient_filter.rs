use crate::{BlurFilter, BlurFilterFlags, Fixed8, Fixed16, GradientRecord, Rectangle, Twips};
use bitflags::bitflags;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GradientFilter {
    pub colors: Vec<GradientRecord>,
    pub blur_x: Fixed16,
    pub blur_y: Fixed16,
    pub angle: Fixed16,
    pub distance: Fixed16,
    pub strength: Fixed8,
    pub flags: GradientFilterFlags,
}

impl GradientFilter {
    #[inline]
    pub fn is_inner(&self) -> bool {
        self.flags.contains(GradientFilterFlags::INNER_SHADOW)
    }

    #[inline]
    pub fn is_knockout(&self) -> bool {
        self.flags.contains(GradientFilterFlags::KNOCKOUT)
    }

    #[inline]
    pub fn is_on_top(&self) -> bool {
        self.flags.contains(GradientFilterFlags::ON_TOP)
    }

    #[inline]
    pub fn num_passes(&self) -> u8 {
        (self.flags & GradientFilterFlags::PASSES).bits()
    }

    pub fn scale(&mut self, x: f32, y: f32) {
        self.blur_x = BlurFilter::scale_blur(self.blur_x, x);
        self.blur_y = BlurFilter::scale_blur(self.blur_y, y);
        self.distance *= Fixed16::from_f32(y);
    }

    pub fn inner_blur_filter(&self) -> BlurFilter {
        BlurFilter {
            blur_x: self.blur_x,
            blur_y: self.blur_y,
            flags: BlurFilterFlags::from_passes(self.num_passes()),
        }
    }

    /// How much room this filter needs beyond the thing it is applied to.
    ///
    /// A gradient glow reaches outside its object exactly as a glow does, and is then displaced by
    /// `distance` along `angle` the way a drop shadow is, so it needs both. Without this the region
    /// reserved is the object's own bounds, and a glow drawn past the edge of that region is cut
    /// off against a straight line -- which is a rectangle around the object, not a glow.
    pub fn calculate_dest_rect(&self, source_rect: Rectangle<Twips>) -> Rectangle<Twips> {
        let mut result = self.inner_blur_filter().calculate_dest_rect(source_rect);
        let distance = self.distance.to_f64();
        let angle = self.angle.to_f64();
        // The renderer samples the blur at both +offset and -offset, so grow in both directions
        // rather than only the one the angle points at.
        let x = Twips::from_pixels((angle.cos() * distance).abs());
        let y = Twips::from_pixels((angle.sin() * distance).abs());
        result.x_min -= x;
        result.x_max += x;
        result.y_min -= y;
        result.y_max += y;
        result
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct GradientFilterFlags: u8 {
        const INNER_SHADOW     = 1 << 7;
        const KNOCKOUT         = 1 << 6;
        const COMPOSITE_SOURCE = 1 << 5;
        const ON_TOP           = 1 << 4;
        const PASSES           = 0b1111;
    }
}

impl GradientFilterFlags {
    #[inline]
    pub fn from_passes(num_passes: u8) -> Self {
        let flags = Self::from_bits_retain(num_passes);
        debug_assert_eq!(flags & Self::PASSES, flags);
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    /// The filter on `UltimateGameClaymore.swf`, an AQW weapon, field for field.
    fn aqw_weapon_glow() -> GradientFilter {
        GradientFilter {
            colors: vec![
                GradientRecord {
                    ratio: 0,
                    color: Color::from_rgba(0x00ff0000),
                },
                GradientRecord {
                    ratio: 255,
                    color: Color::from_rgba(0xffff6600),
                },
            ],
            blur_x: Fixed16::from_f64(17.0),
            blur_y: Fixed16::from_f64(17.0),
            angle: Fixed16::from_f64(4.6425628662109375),
            distance: Fixed16::from_f64(4.0),
            strength: Fixed8::from_f64(1.7890625),
            flags: GradientFilterFlags::COMPOSITE_SOURCE | GradientFilterFlags::from_passes(3),
        }
    }

    /// A gradient glow draws outside the thing it is applied to, so the region reserved for it has
    /// to be bigger than that thing. Returning the source rect unchanged means the glow is cut off
    /// against a straight line on every side, which is a rectangle around the object rather than a
    /// glow, and is what AQW's weapons looked like.
    #[test]
    fn a_gradient_glow_needs_room_outside_its_object() {
        let source = Rectangle {
            x_min: Twips::from_pixels(0.0),
            x_max: Twips::from_pixels(100.0),
            y_min: Twips::from_pixels(0.0),
            y_max: Twips::from_pixels(100.0),
        };
        let grown = aqw_weapon_glow().calculate_dest_rect(source);

        assert!(grown.x_min < source.x_min, "no room on the left");
        assert!(grown.x_max > source.x_max, "no room on the right");
        assert!(grown.y_min < source.y_min, "no room above");
        assert!(grown.y_max > source.y_max, "no room below");
    }

    /// The blur reach is what dominates; the offset only shifts it. Both have to be accounted for,
    /// so the growth must exceed what the blur alone would ask for.
    #[test]
    fn the_offset_is_counted_on_top_of_the_blur() {
        let source = Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(100.0),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(100.0),
        };
        let filter = aqw_weapon_glow();
        let blur_only = filter.inner_blur_filter().calculate_dest_rect(source);
        let grown = filter.calculate_dest_rect(source);

        assert!(grown.width() > blur_only.width());
        assert!(grown.height() > blur_only.height());
    }
}
