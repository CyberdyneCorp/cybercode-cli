//! Color values, blending, WCAG contrast and depth reduction (`tui` → Theme tokens,
//! Color capability and NO_COLOR).

use ratatui::style::Color;

/// How many colors the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    /// `NO_COLOR` or `tui.color: none`: no foreground or background at all.
    None,
}

/// `#rrggbb`, or one of the 16 ANSI names, or `none`/`default` for the terminal's own.
pub fn parse(value: &str) -> Option<Color> {
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let n = u32::from_str_radix(hex, 16).ok()?;
        return Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    let named = match value {
        "none" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" => Color::Gray,
        "darkgray" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    };
    Some(named)
}

/// `from` moved `amount` (0..1) of the way toward `toward`. Colors that are not RGB
/// (the terminal's own palette) cannot be mixed, so the nearer endpoint is kept.
pub fn blend(from: Color, toward: Color, amount: f32) -> Color {
    match (from, toward) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |a: u8, b: u8| {
                (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8
            };
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ if amount < 0.5 => from,
        _ => toward,
    }
}

/// WCAG relative luminance, or `None` for a color the terminal defines.
pub fn luminance(color: Color) -> Option<f64> {
    let Color::Rgb(r, g, b) = color else {
        return None;
    };
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b))
}

/// WCAG contrast ratio between two RGB colors.
pub fn contrast(a: Color, b: Color) -> Option<f64> {
    let (la, lb) = (luminance(a)?, luminance(b)?);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

/// `color` as the terminal at `depth` can show it.
pub fn reduce(color: Color, depth: Depth) -> Color {
    match (depth, color) {
        (Depth::None, _) => Color::Reset,
        (Depth::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
        (Depth::Ansi16, Color::Rgb(r, g, b)) => nearest_16(r, g, b),
        _ => color,
    }
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).unsigned_abs().pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The nearest xterm-256 color among the 6×6×6 cube and the gray ramp. Indexes 0–15
/// are left out because terminals redefine them.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |c: u8| {
        (0..6)
            .min_by_key(|&i| (i32::from(CUBE[i]) - i32::from(c)).abs())
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gray_step = ((average.saturating_sub(8) + 5) / 10).min(23) as u8;
    let gray = 8 + 10 * gray_step;
    if distance((gray, gray, gray), (r, g, b)) < distance(cube, (r, g, b)) {
        232 + gray_step
    } else {
        cube_index as u8
    }
}

/// xterm's default values for the 16 ANSI colors.
const ANSI: [(Color, (u8, u8, u8)); 16] = [
    (Color::Black, (0, 0, 0)),
    (Color::Red, (205, 0, 0)),
    (Color::Green, (0, 205, 0)),
    (Color::Yellow, (205, 205, 0)),
    (Color::Blue, (0, 0, 238)),
    (Color::Magenta, (205, 0, 205)),
    (Color::Cyan, (0, 205, 205)),
    (Color::Gray, (229, 229, 229)),
    (Color::DarkGray, (127, 127, 127)),
    (Color::LightRed, (255, 0, 0)),
    (Color::LightGreen, (0, 255, 0)),
    (Color::LightYellow, (255, 255, 0)),
    (Color::LightBlue, (92, 92, 255)),
    (Color::LightMagenta, (255, 0, 255)),
    (Color::LightCyan, (0, 255, 255)),
    (Color::White, (255, 255, 255)),
];

pub fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    ANSI.iter()
        .min_by_key(|(_, rgb)| distance(*rgb, (r, g, b)))
        .map_or(Color::Reset, |(color, _)| *color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_256_mapping_picks_the_nearest_cube_or_gray_index() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(0x00, 0xe5, 0xff), 45);
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
        assert_eq!(nearest_256(0x0b, 0x0f, 0x14), 233);
        assert_eq!(
            reduce(Color::Rgb(255, 0, 0), Depth::Ansi256),
            Color::Indexed(196)
        );
        assert_eq!(
            reduce(Color::Rgb(250, 10, 10), Depth::Ansi16),
            Color::LightRed
        );
        assert_eq!(reduce(Color::Cyan, Depth::Ansi256), Color::Cyan);
        assert_eq!(reduce(Color::Rgb(1, 2, 3), Depth::None), Color::Reset);
    }

    #[test]
    fn values_parse_hex_and_ansi_names_only() {
        assert_eq!(parse("#00e5ff"), Some(Color::Rgb(0, 0xe5, 0xff)));
        assert_eq!(parse("darkgray"), Some(Color::DarkGray));
        assert_eq!(parse("none"), Some(Color::Reset));
        for bad in ["#fff", "#gggggg", "00e5ff", "\u{1b}[31m", ""] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn contrast_matches_the_wcag_formula() {
        let white = Color::Rgb(255, 255, 255);
        let black = Color::Rgb(0, 0, 0);
        assert!((contrast(white, black).unwrap() - 21.0).abs() < 1e-9);
        assert_eq!(contrast(Color::Reset, black), None);
        assert_eq!(blend(black, white, 0.5), Color::Rgb(128, 128, 128));
        assert_eq!(blend(Color::Reset, white, 0.06), Color::Reset);
    }
}
