//! Status text rendered on the CPU into a grayscale panel, drawn as one more tile-style quad.

use decoder::Gray8Buf;
use font8x8::{UnicodeFonts, BASIC_FONTS, LATIN_FONTS};

const PANEL: u8 = 28;
const INK: u8 = 255;
const PAD: u32 = 6;
const GLYPH: u32 = 8;

/// Panel size in pixels for `lines` at `scale`, without rendering it.
pub fn text_size(lines: &[String], scale: u32) -> (u32, u32) {
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u32;
    (
        (cols * GLYPH * scale + 2 * PAD * scale).max(1),
        (lines.len() as u32 * (GLYPH + 2) * scale + 2 * PAD * scale).max(1),
    )
}

/// White text on a dark panel. `scale` multiplies the 8x8 glyphs (use 2 on high-DPI screens).
pub fn render_text(lines: &[String], scale: u32) -> Gray8Buf {
    render_styled(lines, scale, PANEL)
}

/// Like [`render_text`] with a chosen background gray (buttons use lighter panels).
pub fn render_styled(lines: &[String], scale: u32, background: u8) -> Gray8Buf {
    let (w, h) = text_size(lines, scale);
    let mut data = vec![background; (w * h) as usize];
    for (row, line) in lines.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            // ASCII, then Latin-1 letters (accented names in metadata), else a question mark.
            let Some(glyph) = BASIC_FONTS
                .get(ch)
                .or_else(|| LATIN_FONTS.get(ch))
                .or_else(|| BASIC_FONTS.get('?'))
            else {
                continue;
            };
            let (ox, oy) = (
                (PAD + col as u32 * GLYPH) * scale,
                (PAD + row as u32 * (GLYPH + 2)) * scale,
            );
            for (gy, bits) in glyph.iter().enumerate() {
                for gx in 0..8u32 {
                    if bits >> gx & 1 == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        let y = oy + gy as u32 * scale + sy;
                        for sx in 0..scale {
                            data[(y * w + ox + gx * scale + sx) as usize] = INK;
                        }
                    }
                }
            }
        }
    }
    Gray8Buf { w, h, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_size_follows_text() {
        let b = render_text(&["abc".into(), "de".into()], 2);
        assert_eq!(b.w, (3 * 8 + 12) * 2);
        assert_eq!(b.h, (2 * 10 + 12) * 2);
        assert_eq!(b.data.len(), (b.w * b.h) as usize);
    }

    #[test]
    fn text_size_matches_rendered_size() {
        let lines = ["Open file (O)".to_string()];
        let b = render_styled(&lines, 3, 60);
        assert_eq!(text_size(&lines, 3), (b.w, b.h));
        assert_eq!(b.data[0], 60);
    }

    #[test]
    fn glyphs_draw_ink_and_blank_is_panel() {
        let ink = render_text(&["I".into()], 1)
            .data
            .iter()
            .filter(|&&p| p == INK)
            .count();
        assert!(ink > 5);
        let blank = render_text(&[" ".into()], 1);
        assert!(blank.data.iter().all(|&p| p == PANEL));
    }

    #[test]
    fn latin_letters_get_their_own_glyph_and_others_a_question_mark() {
        let px = |s: &str| render_text(&[s.to_string()], 1).data;
        assert_ne!(
            px("é"),
            px("?"),
            "e-acute should not render as a question mark"
        );
        assert_ne!(px("é"), px("e"));
        assert_eq!(
            px("✓"),
            px("?"),
            "characters with no glyph fall back to '?'"
        );
    }

    #[test]
    fn empty_and_non_ascii_do_not_panic() {
        let _ = render_text(&[], 1);
        let _ = render_text(&["é✓".into()], 3);
    }
}
