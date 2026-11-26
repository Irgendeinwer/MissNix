//! Perceptual OKLab / OKLCH Material 3 ("Material You") tonal palette generator.
//!
//! Computes dark and light surface hierarchies, outlines, text contrast levels,
//! primary/secondary containers, and accent tokens from any `#RRGGBB` seed color
//! or curated preset — eliminating static purple/indigo color casts across themes.

/// Maximum valid preset index (`0..=6` built-in presets, `7` = custom `#RRGGBB` seed).
pub const MAX_ACCENT_PRESET: u8 = 7;

/// Custom accent preset index (`7`).
pub const CUSTOM_ACCENT_PRESET: u8 = 7;

/// Default custom seed hex when none has been configured yet.
pub const DEFAULT_CUSTOM_ACCENT_HEX: &str = "#6366F1";

/// Complete Material 3 tonal palette resolved to 8-bit sRGB `[R, G, B]` triples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePalette {
    pub seed_hex: String,
    pub custom_swatch: [u8; 3],

    pub surface: [u8; 3],
    pub surface_rail: [u8; 3],
    pub surface_container_low: [u8; 3],
    pub surface_container: [u8; 3],
    pub surface_container_high: [u8; 3],
    pub surface_container_highest: [u8; 3],

    pub on_surface: [u8; 3],
    pub on_surface_variant: [u8; 3],
    pub on_surface_muted: [u8; 3],

    pub outline: [u8; 3],
    pub outline_variant: [u8; 3],

    pub primary: [u8; 3],
    pub on_primary: [u8; 3],
    pub primary_container: [u8; 3],
    pub on_primary_container: [u8; 3],

    pub secondary_container: [u8; 3],
    pub on_secondary_container: [u8; 3],
}

/// Parse `#RRGGBB`, `RRGGBB`, `#RGB`, or `RGB` hex strings into `(R, G, B)`.
pub fn parse_hex_color(raw: &str) -> Option<(u8, u8, u8)> {
    let hex = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    match hex.len() {
        6 if hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        3 if hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some((r, g, b))
        }
        _ => None,
    }
}

/// Normalize a user hex input to uppercase `#RRGGBB` if valid.
pub fn normalize_hex_color(raw: &str) -> Option<String> {
    parse_hex_color(raw).map(format_hex_color)
}

/// Format an `(R, G, B)` tuple as uppercase `#RRGGBB`.
pub fn format_hex_color((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Return the canonical seed RGB color for built-in presets `0..=6`.
pub fn preset_seed_rgb(preset: u8) -> (u8, u8, u8) {
    match preset {
        0 => (139, 92, 246),  // Iris Violet   (#8B5CF6)
        1 => (56, 189, 248),  // Nord Frost    (#38BDF8)
        2 => (16, 185, 129),  // Emerald Sage  (#10B981)
        3 => (245, 158, 11),  // Amber Gold    (#F59E0B)
        4 => (244, 63, 94),   // Coral Rose    (#F43F5E)
        5 => (234, 108, 52),  // Sunset Copper (#EA6C34)
        6 => (140, 144, 150), // Graphite Mono (#8C9096)
        _ => (139, 92, 246),
    }
}

/// Resolve a complete dark or light Material 3 palette for `accent_preset` + `custom_accent_hex`.
pub fn resolve_theme_palette(
    accent_preset: u8,
    custom_accent_hex: &str,
    dark_mode: bool,
) -> ThemePalette {
    let custom_rgb = parse_hex_color(custom_accent_hex).unwrap_or((99, 102, 241));
    let seed = if accent_preset == CUSTOM_ACCENT_PRESET {
        custom_rgb
    } else {
        preset_seed_rgb(accent_preset)
    };
    let mut palette = generate_m3_palette(seed, dark_mode);
    let (_, cs, ch) = rgb_to_oklch(custom_rgb);
    palette.custom_swatch = if cs < 0.018 {
        oklch_to_rgb(0.76, 0.0, 0.0)
    } else {
        oklch_to_rgb(0.78, cs.clamp(0.11, 0.17), ch)
    };
    palette
}

/// Generate a perceptually balanced Material 3 tonal palette from any `(R, G, B)` seed.
pub fn generate_m3_palette(seed_rgb: (u8, u8, u8), dark_mode: bool) -> ThemePalette {
    let (_l_seed, c_seed, h) = rgb_to_oklch(seed_rgb);
    let monochrome = c_seed < 0.018;

    // Subtle Material 3 neutral chroma tinted toward the seed hue (zero for Graphite/gray seeds).
    let nc = if monochrome {
        0.0
    } else {
        (c_seed * 0.115).clamp(0.012, 0.022)
    };

    let seed_hex = format_hex_color(seed_rgb);
    let custom_swatch = [seed_rgb.0, seed_rgb.1, seed_rgb.2];

    if dark_mode {
        let pc = if monochrome {
            0.0
        } else {
            (c_seed * 0.88).clamp(0.095, 0.158)
        };
        let sc = if monochrome {
            0.0
        } else {
            (nc * 1.65).min(0.034)
        };

        ThemePalette {
            seed_hex,
            custom_swatch,

            surface: oklch_to_rgb(0.178, nc * 0.82, h),
            surface_rail: oklch_to_rgb(0.200, nc * 0.92, h),
            surface_container_low: oklch_to_rgb(0.214, nc * 0.96, h),
            surface_container: oklch_to_rgb(0.234, nc, h),
            surface_container_high: oklch_to_rgb(0.272, nc * 1.12, h),
            surface_container_highest: oklch_to_rgb(0.312, nc * 1.22, h),

            on_surface: oklch_to_rgb(0.946, nc * 0.45, h),
            on_surface_variant: oklch_to_rgb(0.748, nc * 0.95, h),
            on_surface_muted: oklch_to_rgb(0.585, nc * 0.85, h),

            outline: oklch_to_rgb(0.445, nc * 1.15, h),
            outline_variant: oklch_to_rgb(0.32, nc * 0.95, h),

            primary: oklch_to_rgb(0.822, pc, h),
            on_primary: oklch_to_rgb(0.235, pc * 0.52, h),
            primary_container: oklch_to_rgb(0.335, pc * 0.66, h),
            on_primary_container: oklch_to_rgb(0.915, pc * 0.48, h),

            secondary_container: oklch_to_rgb(0.285, sc, h),
            on_secondary_container: oklch_to_rgb(0.900, sc * 0.85, h),
        }
    } else {
        let pc = if monochrome {
            0.0
        } else {
            c_seed.clamp(0.105, 0.178)
        };
        let sc = if monochrome {
            0.0
        } else {
            (nc * 1.50).min(0.030)
        };

        ThemePalette {
            seed_hex,
            custom_swatch,

            surface: oklch_to_rgb(0.978, nc * 0.42, h),
            surface_rail: oklch_to_rgb(0.948, nc * 0.72, h),
            surface_container_low: oklch_to_rgb(0.963, nc * 0.55, h),
            surface_container: oklch_to_rgb(0.995, nc * 0.18, h),
            surface_container_high: oklch_to_rgb(0.928, nc * 0.85, h),
            surface_container_highest: oklch_to_rgb(0.896, nc * 1.02, h),

            on_surface: oklch_to_rgb(0.205, nc * 0.75, h),
            on_surface_variant: oklch_to_rgb(0.435, nc * 1.05, h),
            on_surface_muted: oklch_to_rgb(0.562, nc * 0.90, h),

            outline: oklch_to_rgb(0.785, nc * 0.95, h),
            outline_variant: oklch_to_rgb(0.885, nc * 0.75, h),

            primary: oklch_to_rgb(0.468, pc, h),
            on_primary: [255, 255, 255],
            primary_container: oklch_to_rgb(0.912, pc * 0.46, h),
            on_primary_container: oklch_to_rgb(0.238, pc * 0.65, h),

            secondary_container: oklch_to_rgb(0.914, sc, h),
            on_secondary_container: oklch_to_rgb(0.255, sc * 1.05, h),
        }
    }
}

fn srgb_to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn rgb_to_oklch((r, g, b): (u8, u8, u8)) -> (f32, f32, f32) {
    let lr = srgb_to_linear(r);
    let lg = srgb_to_linear(g);
    let lb = srgb_to_linear(b);

    let l = (0.412_221_46 * lr + 0.536_332_55 * lg + 0.051_445_995 * lb).cbrt();
    let m = (0.211_903_5 * lr + 0.680_699_5 * lg + 0.107_396_96 * lb).cbrt();
    let s = (0.088_302_46 * lr + 0.281_718_85 * lg + 0.629_978_7 * lb).cbrt();

    let ok_l = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let ok_a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let ok_b = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;

    let chroma = ok_a.hypot(ok_b);
    let hue = ok_b.atan2(ok_a);
    (ok_l, chroma, hue)
}

fn oklab_to_linear_rgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    (r, g, bl)
}

/// Convert OKLCH `(L, C, h)` to 8-bit sRGB while preserving perceptual lightness `L` and hue `h`
/// by binary-searching chroma onto the sRGB gamut boundary whenever needed.
fn oklch_to_rgb(lightness: f32, chroma: f32, hue: f32) -> [u8; 3] {
    let l = lightness.clamp(0.0, 1.0);
    if chroma <= 0.000_1 {
        let v = linear_to_srgb_u8(oklab_to_linear_rgb(l, 0.0, 0.0).0);
        return [v, v, v];
    }

    let cos_h = hue.cos();
    let sin_h = hue.sin();
    let mut lo = 0.0_f32;
    let mut hi = chroma;

    for _ in 0..14 {
        let mid = 0.5 * (lo + hi);
        let (r, g, b) = oklab_to_linear_rgb(l, mid * cos_h, mid * sin_h);
        if (-0.000_5..=1.000_5).contains(&r)
            && (-0.000_5..=1.000_5).contains(&g)
            && (-0.000_5..=1.000_5).contains(&b)
        {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    let (r, g, b) = oklab_to_linear_rgb(l, lo * cos_h, lo * sin_h);
    [
        linear_to_srgb_u8(r),
        linear_to_srgb_u8(g),
        linear_to_srgb_u8(b),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_hex_colors() {
        assert_eq!(parse_hex_color("#ea6c34"), Some((234, 108, 52)));
        assert_eq!(parse_hex_color("38BDF8"), Some((56, 189, 248)));
        assert_eq!(parse_hex_color("#f80"), Some((255, 136, 0)));
        assert_eq!(
            normalize_hex_color("  #6ee7b7 "),
            Some("#6EE7B7".to_string())
        );
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("not-a-hex"), None);
    }

    #[test]
    fn copper_and_emerald_surfaces_have_no_purple_cast() {
        let copper_dark = resolve_theme_palette(5, DEFAULT_CUSTOM_ACCENT_HEX, true);
        assert!(
            copper_dark.surface[0] > copper_dark.surface[2],
            "Sunset Copper dark surface should be warm (R > B), got {:?}",
            copper_dark.surface
        );
        assert!(
            copper_dark.surface_container[0] > copper_dark.surface_container[2],
            "Sunset Copper container should be warm (R > B), got {:?}",
            copper_dark.surface_container
        );
        assert!(copper_dark.primary[0] > copper_dark.primary[2]);

        let emerald_dark = resolve_theme_palette(2, DEFAULT_CUSTOM_ACCENT_HEX, true);
        assert!(
            emerald_dark.surface_container[1] > emerald_dark.surface_container[0]
                && emerald_dark.surface_container[1] > emerald_dark.surface_container[2],
            "Emerald container should be sage-tinted (G > R, B), got {:?}",
            emerald_dark.surface_container
        );

        let graphite = resolve_theme_palette(6, DEFAULT_CUSTOM_ACCENT_HEX, true);
        assert_eq!(graphite.surface[0], graphite.surface[1]);
        assert_eq!(graphite.surface[1], graphite.surface[2]);
    }

    #[test]
    fn custom_hex_generates_distinct_dark_and_light_palettes() {
        let custom_dark = resolve_theme_palette(CUSTOM_ACCENT_PRESET, "#E11D48", true);
        let custom_light = resolve_theme_palette(CUSTOM_ACCENT_PRESET, "#E11D48", false);
        assert_eq!(custom_dark.seed_hex, "#E11D48");
        assert!(custom_dark.primary[0] > custom_dark.primary[1]);
        assert!(custom_dark.surface[0] < custom_light.surface[0]);
        assert!(custom_dark.on_surface[0] > custom_light.on_surface[0]);
    }
}
