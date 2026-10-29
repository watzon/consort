//! Colors in sRGB, WCAG contrast, and OKLCH lightness shifts.

use std::fmt;

use serde::{Deserialize, Deserializer, de};

/// An opaque sRGB color, as a theme file writes it (`#RRGGBB`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// An sRGB color with alpha. Derived colors (tints, scrims) have alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub rgb: Rgb,
    pub a: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    /// Parses `#RRGGBB`. Returns `None` for any other form.
    pub fn parse(text: &str) -> Option<Rgb> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(Rgb::from_u32(value))
    }

    pub const fn from_u32(value: u32) -> Rgb {
        Rgb {
            r: ((value >> 16) & 0xFF) as u8,
            g: ((value >> 8) & 0xFF) as u8,
            b: (value & 0xFF) as u8,
        }
    }

    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    pub fn opaque(self) -> Rgba {
        Rgba { rgb: self, a: 255 }
    }

    pub fn with_alpha(self, alpha: f32) -> Rgba {
        Rgba {
            rgb: self,
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        }
    }

    /// The WCAG 2 relative luminance.
    pub fn luminance(self) -> f64 {
        let channel = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// The WCAG 2 contrast ratio between two opaque colors, from 1 to 21.
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (light, dark) = if a > b { (a, b) } else { (b, a) };
        (light + 0.05) / (dark + 0.05)
    }

    /// The color as OKLCH: lightness (0 to 1), chroma and hue in degrees.
    pub fn to_oklch(self) -> (f64, f64, f64) {
        let (l, a, b) = self.to_oklab();
        let c = (a * a + b * b).sqrt();
        let h = b.atan2(a).to_degrees();
        (l, c, h)
    }

    pub fn from_oklch(l: f64, c: f64, h: f64) -> Rgb {
        let h = h.to_radians();
        Rgb::from_oklab(l, c * h.cos(), c * h.sin())
    }

    /// Moves the OKLCH lightness by `delta` and keeps chroma and hue.
    pub fn shift_lightness(self, delta: f64) -> Rgb {
        let (l, c, h) = self.to_oklch();
        Rgb::from_oklch((l + delta).clamp(0.0, 1.0), c, h)
    }

    fn to_oklab(self) -> (f64, f64, f64) {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(self.r), lin(self.g), lin(self.b));
        let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
        let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
        let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        (
            0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        )
    }

    fn from_oklab(l: f64, a: f64, b: f64) -> Rgb {
        let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
        let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
        let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        let r = 4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s;
        let g = -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s;
        let b = -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s;
        let encode = |c: f64| {
            let c = c.clamp(0.0, 1.0);
            let c = if c <= 0.003_130_8 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (c * 255.0).round().clamp(0.0, 255.0) as u8
        };
        Rgb {
            r: encode(r),
            g: encode(g),
            b: encode(b),
        }
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:02X}", self.rgb, self.a)
    }
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        rgb: Rgb::BLACK,
        a: 0,
    };

    /// Parses `#RRGGBB` or `#RRGGBBAA`.
    pub fn parse(text: &str) -> Option<Rgba> {
        let hex = text.strip_prefix('#')?;
        match hex.len() {
            6 => Rgb::parse(text).map(Rgb::opaque),
            8 => {
                let rgb = Rgb::parse(&text[..7])?;
                let a = u8::from_str_radix(&hex[6..], 16).ok()?;
                Some(Rgba { rgb, a })
            }
            _ => None,
        }
    }

    pub fn alpha(self) -> f32 {
        f32::from(self.a) / 255.0
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Rgb::parse(&text).ok_or_else(|| {
            de::Error::custom(format!(
                "\"{text}\" is not a color. Write colors as #RRGGBB"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_hex() {
        let c = Rgb::parse("#E7be62").expect("valid");
        assert_eq!(c.to_string(), "#E7BE62");
        assert!(Rgb::parse("E7BE62").is_none());
        assert!(Rgb::parse("#E7BE6").is_none());
        assert_eq!(
            Rgba::parse("#00000073").map(|c| c.a),
            Some(0x73),
            "alpha is the last byte"
        );
    }

    #[test]
    fn contrast_matches_wcag_extremes() {
        let white = Rgb::from_u32(0xFFFFFF);
        assert!((white.contrast(Rgb::BLACK) - 21.0).abs() < 1e-9);
        assert!((white.contrast(white) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn oklch_round_trips() {
        for value in [0x131E1A, 0xE7BE62, 0xF8FCF8, 0x77B4ED, 0x000000, 0xFFFFFF] {
            let c = Rgb::from_u32(value);
            let (l, ch, h) = c.to_oklch();
            assert_eq!(Rgb::from_oklch(l, ch, h), c, "{c}");
        }
    }

    #[test]
    fn lightness_shift_moves_the_right_way() {
        let canvas = Rgb::from_u32(0x131E1A);
        let hover = canvas.shift_lightness(0.035);
        assert!(hover.luminance() > canvas.luminance());
        let light = Rgb::from_u32(0xF8FCF8);
        assert!(light.shift_lightness(-0.035).luminance() < light.luminance());
    }
}
