use gpui::{Pixels, Rgba, WindowAppearance, px, rgb, rgba};

const SERIES_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewerTheme {
    pub colors: ViewerColors,
    pub spacing: ViewerSpacing,
    pub dark: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewerColors {
    pub window: Rgba,
    pub panel: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub element_hover: Rgba,
    pub element_active: Rgba,
    pub focus: Rgba,
    pub disabled: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_text: Rgba,
    pub warning_background: Rgba,
    pub warning_text: Rgba,
    pub tooltip_background: Rgba,
    pub tooltip_text: Rgba,
    pub error_background: Rgba,
    pub error_text: Rgba,
    pub chart_grid: Rgba,
    pub brush_selection: Rgba,
    pub series: [Rgba; SERIES_COUNT],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewerSpacing {
    pub tab_height: Pixels,
    pub tree_row_height: Pixels,
    pub control_height: Pixels,
    pub sidebar_width: Pixels,
    pub panel_padding: Pixels,
    pub content_padding: Pixels,
    pub content_gap: Pixels,
    pub corner_radius: Pixels,
}

impl ViewerTheme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        let dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        Self {
            colors: if dark {
                ViewerColors::dark()
            } else {
                ViewerColors::light()
            },
            spacing: ViewerSpacing::default_density(),
            dark,
        }
    }
}

impl ViewerColors {
    pub fn series_color(self, index: usize) -> Rgba {
        self.series[index % self.series.len()]
    }

    fn light() -> Self {
        Self {
            window: rgb(0xf7f7f8),
            panel: rgb(0xf3f3f4),
            surface: rgb(0xffffff),
            border: rgb(0xd8dadd),
            text: rgb(0x202124),
            text_muted: rgb(0x6b7280),
            element_hover: rgb(0xe8e8ea),
            element_active: rgb(0xdbeafe),
            focus: rgb(0x2563eb),
            disabled: rgb(0x9ca3af),
            accent: rgb(0x2563eb),
            accent_hover: rgb(0x1d4ed8),
            accent_text: rgb(0xffffff),
            warning_background: rgb(0xfef3c7),
            warning_text: rgb(0x78350f),
            tooltip_background: rgb(0x111827),
            tooltip_text: rgb(0xffffff),
            error_background: rgb(0xfee2e2),
            error_text: rgb(0x991b1b),
            chart_grid: rgb(0xe5e7eb),
            brush_selection: rgba(0x2563eb24),
            series: [
                rgb(0x2563eb),
                rgb(0xdc2626),
                rgb(0x059669),
                rgb(0x7c3aed),
                rgb(0xea580c),
                rgb(0x0891b2),
                rgb(0xdb2777),
                rgb(0x65a30d),
                rgb(0x4f46e5),
                rgb(0x9333ea),
            ],
        }
    }

    fn dark() -> Self {
        Self {
            window: rgb(0x181818),
            panel: rgb(0x1f1f1f),
            surface: rgb(0x242424),
            border: rgb(0x3a3a3a),
            text: rgb(0xe5e7eb),
            text_muted: rgb(0x9ca3af),
            element_hover: rgb(0x2d2d2d),
            element_active: rgb(0x343b4b),
            focus: rgb(0x60a5fa),
            disabled: rgb(0x6b7280),
            accent: rgb(0x3b82f6),
            accent_hover: rgb(0x60a5fa),
            accent_text: rgb(0xffffff),
            warning_background: rgb(0x4a3514),
            warning_text: rgb(0xfde68a),
            tooltip_background: rgb(0xf3f4f6),
            tooltip_text: rgb(0x111827),
            error_background: rgb(0x4c1d1d),
            error_text: rgb(0xfca5a5),
            chart_grid: rgb(0x343434),
            brush_selection: rgba(0x60a5fa30),
            series: [
                rgb(0x60a5fa),
                rgb(0xf87171),
                rgb(0x34d399),
                rgb(0xa78bfa),
                rgb(0xfb923c),
                rgb(0x22d3ee),
                rgb(0xf472b6),
                rgb(0xa3e635),
                rgb(0x818cf8),
                rgb(0xc084fc),
            ],
        }
    }
}

impl ViewerSpacing {
    fn default_density() -> Self {
        Self {
            tab_height: px(32.),
            tree_row_height: px(28.),
            control_height: px(28.),
            sidebar_width: px(320.),
            panel_padding: px(12.),
            content_padding: px(20.),
            content_gap: px(12.),
            corner_radius: px(4.),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearances_have_distinct_semantic_palettes() {
        let light = ViewerTheme::for_appearance(WindowAppearance::Light);
        let dark = ViewerTheme::for_appearance(WindowAppearance::Dark);

        assert!(!light.dark);
        assert!(dark.dark);
        assert_ne!(light.colors.window, dark.colors.window);
        assert_ne!(light.colors.text, dark.colors.text);
        assert_eq!(light.spacing, dark.spacing);
    }

    #[test]
    fn first_ten_series_colors_are_distinct_in_both_appearances() {
        for appearance in [WindowAppearance::Light, WindowAppearance::Dark] {
            let colors = ViewerTheme::for_appearance(appearance).colors;
            for left in 0..SERIES_COUNT {
                for right in (left + 1)..SERIES_COUNT {
                    assert_ne!(colors.series_color(left), colors.series_color(right));
                }
            }
        }
    }

    #[test]
    fn default_density_matches_the_pinned_reference_geometry() {
        let spacing = ViewerSpacing::default_density();

        assert_eq!(spacing.tab_height, px(32.));
        assert_eq!(spacing.tree_row_height, px(28.));
        assert_eq!(spacing.control_height, px(28.));
    }
}
