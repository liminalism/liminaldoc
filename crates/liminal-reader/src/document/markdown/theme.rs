//! Markdown reader theme, ported from `MarkdownViewer.Wpf`'s `Theme`.
//!
//! Every color below is the WPF viewer's light or dark value verbatim, and the
//! type sizes are its pixel sizes: the Markdown page is laid out at 96 dpi
//! (Letter at 816×1056 units), so WPF pixels transfer one-to-one with no
//! conversion drift. Warm Paper reuses the light theme on the viewer's warm
//! paper color; the decorative Sanzo palettes never recolor page pixels.

use crate::document::ColorMode;

/// Document units per inch for Markdown pages. Matches the WPF viewer's
/// 96-dpi pixels, so its sizes and paddings transfer exactly.
pub const UNITS_PER_INCH: f64 = 96.0;
/// Letter page width at [`UNITS_PER_INCH`].
pub const PAGE_WIDTH: f64 = 816.0;
/// Letter page height at [`UNITS_PER_INCH`].
pub const PAGE_HEIGHT: f64 = 1056.0;
/// Left and right page margins.
pub const MARGIN_X: f64 = 72.0;
/// Top and bottom page margins.
pub const MARGIN_Y: f64 = 64.0;

/// Body text size (WPF `FlowDocument.FontSize`).
pub const BODY_SIZE: f32 = 15.0;
/// Code-block text size.
pub const CODE_SIZE: f32 = 13.0;
/// HTML-block text size.
pub const HTML_SIZE: f32 = 12.0;
/// Heading sizes H1–H6 (WPF `BuildHeading`).
pub const HEADING_SIZES: [f32; 6] = [30.0, 24.0, 20.0, 17.0, 15.0, 14.0];
/// Line-height factor, shared with the chrome text renderer.
pub const LINE_HEIGHT_FACTOR: f32 = 1.35;

/// Space after a paragraph (WPF paragraph margin).
pub const PARAGRAPH_GAP: f64 = 10.0;
/// Space above an H1 heading; other headings use [`OTHER_HEADING_TOP`].
pub const H1_TOP: f64 = 4.0;
/// Space above H2–H6 headings.
pub const OTHER_HEADING_TOP: f64 = 18.0;
/// Space below a heading.
pub const HEADING_BOTTOM: f64 = 6.0;
/// Padding under an H1/H2 rule.
pub const HEADING_RULE_PADDING: f64 = 4.0;
/// Quote bar width and the padding after it.
pub const QUOTE_BAR: f64 = 4.0;
pub const QUOTE_PADDING: f64 = 12.0;
/// Quote block vertical padding and bottom margin.
pub const QUOTE_PAD_Y: f64 = 4.0;
pub const QUOTE_GAP: f64 = 10.0;
/// Nested list indentation (WPF list padding).
pub const LIST_INDENT: f64 = 28.0;
/// Paragraph margin inside list items and table cells.
pub const TIGHT_PARAGRAPH_GAP: f64 = 2.0;
/// Space after a list.
pub const LIST_GAP: f64 = 10.0;
/// Code-block padding and vertical margins.
pub const CODE_PAD_X: f64 = 12.0;
pub const CODE_PAD_Y: f64 = 10.0;
pub const CODE_TOP: f64 = 4.0;
pub const CODE_BOTTOM: f64 = 12.0;
/// Table cell padding.
pub const CELL_PAD_X: f64 = 8.0;
pub const CELL_PAD_Y: f64 = 4.0;
/// Space below a table.
pub const TABLE_GAP: f64 = 14.0;
/// Rule vertical margins.
pub const RULE_TOP: f64 = 6.0;
pub const RULE_BOTTOM: f64 = 10.0;

/// The WPF viewer's color table, one theme per appearance.
#[derive(Debug, Clone, Copy)]
pub struct MdPalette {
    /// Page background.
    pub paper: u32,
    /// Body text.
    pub ink: u32,
    /// Quotes, HTML, image alt text.
    pub muted: u32,
    /// Links.
    pub link: u32,
    /// Code-block background.
    pub code_bg: u32,
    /// Inline-code background.
    pub inline_code_bg: u32,
    /// Quote bar and table/header rules.
    pub rule: u32,
    /// Table header row background.
    pub table_header_bg: u32,
}

impl MdPalette {
    /// The WPF light theme.
    pub const LIGHT: Self = Self {
        paper: 0x00FD_FDFD,
        ink: 0x001E_1E1E,
        muted: 0x0060_6060,
        link: 0x000A_66CC,
        code_bg: 0x00F4_F4F4,
        inline_code_bg: 0x00EC_ECEC,
        rule: 0x00D0_D0D0,
        table_header_bg: 0x00F0_F0F0,
    };

    /// The WPF dark theme.
    pub const DARK: Self = Self {
        paper: 0x001E_1E1E,
        ink: 0x00F1_F1F1,
        muted: 0x00A0_A0A0,
        link: 0x0066_AFFF,
        code_bg: 0x002A_2A2A,
        inline_code_bg: 0x0033_3333,
        rule: 0x0040_4040,
        table_header_bg: 0x002A_2A2A,
    };

    /// The viewer's Warm Paper page color under the light theme.
    pub const WARM_PAPER: u32 = 0x00F2_E8D2;
}

/// Resolve the palette for a tile's color mode.
pub fn palette_for(color_mode: ColorMode) -> MdPalette {
    match color_mode {
        ColorMode::Night => MdPalette::DARK,
        ColorMode::WarmPaper => MdPalette {
            paper: MdPalette::WARM_PAPER,
            ..MdPalette::LIGHT
        },
        ColorMode::Original | ColorMode::SanzoEarth | ColorMode::SanzoSea => MdPalette::LIGHT,
    }
}

/// Heading size for a 1–6 level, clamped like the WPF builder.
pub fn heading_size(level: u8) -> f32 {
    let index = level.clamp(1, 6) as usize - 1;
    HEADING_SIZES[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_follow_the_color_mode() {
        assert_eq!(palette_for(ColorMode::Original).paper, 0x00FD_FDFD);
        assert_eq!(palette_for(ColorMode::Night).paper, 0x001E_1E1E);
        assert_eq!(
            palette_for(ColorMode::WarmPaper).paper,
            MdPalette::WARM_PAPER
        );
        assert_eq!(palette_for(ColorMode::WarmPaper).ink, MdPalette::LIGHT.ink);
    }

    #[test]
    fn heading_sizes_match_the_wpf_scale() {
        assert_eq!(
            (1..=6).map(heading_size).collect::<Vec<_>>(),
            vec![30.0, 24.0, 20.0, 17.0, 15.0, 14.0]
        );
        assert_eq!(heading_size(0), 30.0);
        assert_eq!(heading_size(9), 14.0);
    }
}
