use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Id, Image, Response, Shadow, Stroke, TextStyle, Vec2};

use crate::reading::AppTheme;

pub const TOOLBAR_HEIGHT: f32 = 48.0;
pub const TAB_HEIGHT: f32 = 34.0;
pub const TAB_WIDTH: f32 = 220.0;
pub const STATUS_HEIGHT: f32 = 32.0;
pub const STATUS_VERTICAL_PADDING: i8 = 5;
pub const SIDEBAR_WIDTH: f32 = 260.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 220.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 420.0;
pub const SETTINGS_WIDTH: f32 = 300.0;
pub const CONTROL_RADIUS: u8 = 8;
pub const CARD_RADIUS: u8 = 12;
pub const LIBRARY_CARD_WIDTH: f32 = 184.0;
pub const LIBRARY_CARD_HEIGHT: f32 = 354.0;
pub const MOTION_FAST: f32 = 0.12;
pub const MOTION_CARD: f32 = 0.16;
pub const MOTION_SELECTION: f32 = 0.18;
pub const MOTION_DRAWER: f32 = 0.22;

pub fn ui_font(size: f32) -> FontId { FontId::new(size, FontFamily::Name("koiflow-sans".into())) }
pub fn display_font(size: f32) -> FontId { FontId::new(size, FontFamily::Name("koiflow-serif".into())) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutClass { Small, Medium, Large }

impl LayoutClass {
    pub fn for_width(width: f32) -> Self {
        if width < 760.0 { Self::Small } else if width < 1180.0 { Self::Medium } else { Self::Large }
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub app_bg: Color32,
    pub panel: Color32,
    pub elevated: Color32,
    pub canvas: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub danger: Color32,
    pub surface_low: Color32,
    pub surface_high: Color32,
    pub separator: Color32,
    pub focus: Color32,
    pub disabled: Color32,
    pub shadow: Color32,
}

impl Palette {
    pub fn for_theme(theme: AppTheme) -> Self {
        match theme {
            AppTheme::Light => Self {
                app_bg: Color32::from_rgb(238, 246, 241), panel: Color32::from_rgb(247, 251, 248),
                elevated: Color32::WHITE, canvas: Color32::from_rgb(233, 239, 235),
                text: Color32::from_rgb(24, 53, 41), muted: Color32::from_rgb(80, 107, 93),
                border: Color32::from_rgb(198, 224, 210), accent: Color32::from_rgb(36, 107, 79),
                accent_soft: Color32::from_rgb(216, 241, 226), danger: Color32::from_rgb(176, 70, 65),
                surface_low: Color32::from_rgb(242, 248, 244), surface_high: Color32::from_rgb(252, 254, 252), separator: Color32::from_rgb(210, 228, 218),
                focus: Color32::from_rgb(25, 122, 82), disabled: Color32::from_rgb(145, 164, 154), shadow: Color32::from_black_alpha(28),
            },
            AppTheme::Dark => Self {
                app_bg: Color32::from_rgb(14, 35, 25), panel: Color32::from_rgb(20, 48, 34),
                elevated: Color32::from_rgb(29, 62, 45), canvas: Color32::from_rgb(10, 28, 20),
                text: Color32::from_rgb(239, 249, 242), muted: Color32::from_rgb(184, 210, 194),
                border: Color32::from_rgb(53, 98, 77), accent: Color32::from_rgb(177, 227, 197),
                accent_soft: Color32::from_rgb(42, 81, 59), danger: Color32::from_rgb(236, 139, 126),
                surface_low: Color32::from_rgb(16, 41, 29), surface_high: Color32::from_rgb(34, 69, 51), separator: Color32::from_rgb(45, 82, 64),
                focus: Color32::from_rgb(153, 231, 185), disabled: Color32::from_rgb(105, 130, 115), shadow: Color32::from_black_alpha(86),
            },
            AppTheme::White => Self {
                app_bg: Color32::WHITE, panel: Color32::WHITE, elevated: Color32::WHITE, canvas: Color32::WHITE,
                text: Color32::from_rgb(20, 24, 22), muted: Color32::from_rgb(78, 84, 81),
                border: Color32::from_rgb(216, 221, 218), accent: Color32::from_rgb(31, 99, 72),
                accent_soft: Color32::from_rgb(225, 241, 232), danger: Color32::from_rgb(169, 51, 47),
                surface_low: Color32::WHITE, surface_high: Color32::WHITE, separator: Color32::from_rgb(226, 229, 227),
                focus: Color32::from_rgb(20, 112, 76), disabled: Color32::from_rgb(151, 156, 153), shadow: Color32::from_black_alpha(22),
            },
            AppTheme::OffWhite => Self {
                app_bg: Color32::from_rgb(246, 244, 238), panel: Color32::from_rgb(251, 250, 246),
                elevated: Color32::from_rgb(255, 254, 250), canvas: Color32::from_rgb(239, 237, 230),
                text: Color32::from_rgb(38, 40, 36), muted: Color32::from_rgb(91, 94, 87),
                border: Color32::from_rgb(211, 208, 198), accent: Color32::from_rgb(51, 99, 76),
                accent_soft: Color32::from_rgb(224, 235, 226), danger: Color32::from_rgb(166, 62, 55),
                surface_low: Color32::from_rgb(248, 247, 242), surface_high: Color32::from_rgb(255, 254, 250), separator: Color32::from_rgb(220, 217, 207),
                focus: Color32::from_rgb(45, 108, 77), disabled: Color32::from_rgb(151, 150, 143), shadow: Color32::from_black_alpha(26),
            },
            AppTheme::Sepia => Self {
                app_bg: Color32::from_rgb(244, 237, 222), panel: Color32::from_rgb(252, 248, 239),
                elevated: Color32::from_rgb(255, 253, 247), canvas: Color32::from_rgb(232, 223, 201),
                text: Color32::from_rgb(54, 46, 35), muted: Color32::from_rgb(100, 86, 67),
                border: Color32::from_rgb(205, 188, 157), accent: Color32::from_rgb(61, 91, 70),
                accent_soft: Color32::from_rgb(225, 231, 213), danger: Color32::from_rgb(155, 55, 43),
                surface_low: Color32::from_rgb(247, 242, 229), surface_high: Color32::from_rgb(255, 252, 244), separator: Color32::from_rgb(220, 206, 180),
                focus: Color32::from_rgb(52, 104, 75), disabled: Color32::from_rgb(151, 137, 113), shadow: Color32::from_black_alpha(30),
            },
            AppTheme::Gray => Self {
                app_bg: Color32::from_rgb(224, 224, 224), panel: Color32::from_rgb(236, 236, 236),
                elevated: Color32::from_rgb(247, 247, 247), canvas: Color32::from_rgb(210, 210, 210),
                text: Color32::from_rgb(29, 29, 29), muted: Color32::from_rgb(78, 78, 78),
                border: Color32::from_rgb(174, 174, 174), accent: Color32::from_rgb(55, 55, 55),
                accent_soft: Color32::from_rgb(204, 204, 204), danger: Color32::from_rgb(155, 45, 45),
                surface_low: Color32::from_rgb(230, 230, 230), surface_high: Color32::from_rgb(250, 250, 250), separator: Color32::from_rgb(190, 190, 190),
                focus: Color32::from_rgb(35, 35, 35), disabled: Color32::from_rgb(132, 132, 132), shadow: Color32::from_black_alpha(28),
            },
            AppTheme::Amoled => Self {
                app_bg: Color32::BLACK, panel: Color32::BLACK, elevated: Color32::BLACK, canvas: Color32::BLACK,
                text: Color32::from_rgb(248, 248, 248), muted: Color32::from_rgb(190, 190, 190),
                border: Color32::from_rgb(53, 53, 53), accent: Color32::from_rgb(143, 225, 177),
                accent_soft: Color32::from_rgb(23, 53, 36), danger: Color32::from_rgb(255, 123, 112),
                surface_low: Color32::BLACK, surface_high: Color32::BLACK, separator: Color32::from_rgb(38, 38, 38),
                focus: Color32::from_rgb(143, 225, 177), disabled: Color32::from_rgb(112, 112, 112), shadow: Color32::from_black_alpha(120),
            },
        }
    }

    pub fn blend(self, other: Self, t: f32) -> Self {
        let mix = |a: Color32, b: Color32| {
            let t = t.clamp(0.0, 1.0);
            Color32::from_rgba_unmultiplied(
                (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
                (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
                (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
                (a.a() as f32 + (b.a() as f32 - a.a() as f32) * t).round() as u8,
            )
        };
        Self {
            app_bg: mix(self.app_bg, other.app_bg), panel: mix(self.panel, other.panel), elevated: mix(self.elevated, other.elevated),
            canvas: mix(self.canvas, other.canvas), text: mix(self.text, other.text), muted: mix(self.muted, other.muted), border: mix(self.border, other.border),
            accent: mix(self.accent, other.accent), accent_soft: mix(self.accent_soft, other.accent_soft), danger: mix(self.danger, other.danger),
            surface_low: mix(self.surface_low, other.surface_low), surface_high: mix(self.surface_high, other.surface_high), separator: mix(self.separator, other.separator),
            focus: mix(self.focus, other.focus), disabled: mix(self.disabled, other.disabled), shadow: mix(self.shadow, other.shadow),
        }
    }
}

#[derive(Clone, Copy)]
pub struct ReaderPalette { pub surface: Color32, pub text: Color32, pub muted: Color32, pub quote: Color32, pub code: Color32, pub highlight: Color32 }

impl ReaderPalette {
    pub fn for_theme(theme: crate::reading::ReaderTheme) -> Self {
        match theme {
            crate::reading::ReaderTheme::Paper => Self { surface: Color32::from_rgb(255, 255, 252), text: Color32::from_rgb(31, 43, 37), muted: Color32::from_rgb(97, 107, 101), quote: Color32::from_rgb(42, 103, 76), code: Color32::from_rgb(241, 244, 241), highlight: Color32::from_rgba_unmultiplied(245, 207, 83, 74) },
            crate::reading::ReaderTheme::Sepia => Self { surface: Color32::from_rgb(248, 241, 222), text: Color32::from_rgb(62, 49, 34), muted: Color32::from_rgb(116, 96, 70), quote: Color32::from_rgb(112, 78, 37), code: Color32::from_rgb(235, 224, 198), highlight: Color32::from_rgba_unmultiplied(222, 166, 52, 78) },
            crate::reading::ReaderTheme::Dark => Self { surface: Color32::from_rgb(22, 42, 32), text: Color32::from_rgb(231, 240, 234), muted: Color32::from_rgb(164, 186, 173), quote: Color32::from_rgb(145, 218, 176), code: Color32::from_rgb(31, 57, 43), highlight: Color32::from_rgba_unmultiplied(221, 184, 65, 62) },
        }
    }
}

pub fn animate_bool(ctx: &egui::Context, id: Id, value: bool, seconds: f32, reduced_motion: bool) -> f32 {
    if reduced_motion { if value { 1.0 } else { 0.0 } } else { ctx.animate_bool_with_time_and_easing(id, value, seconds, egui::emath::easing::cubic_out) }
}

pub fn card_shadow(color: Color32, hovered: bool) -> Shadow {
    Shadow { offset: [0, if hovered { 4 } else { 2 }], blur: if hovered { 12 } else { 7 }, spread: 0, color }
}

#[derive(Clone, Copy)]
pub enum Icon {
    Panel, Library, Search, Settings, Open, More, Close, ChevronLeft, ChevronRight,
    ZoomIn, ZoomOut, Rotate, Bookmark, Highlight, Note, Lock,
}

impl Icon {
    fn source(self) -> egui::ImageSource<'static> {
        match self {
            Self::Panel => egui::include_image!("../../assets/icons/panel-left.svg"),
            Self::Library => egui::include_image!("../../assets/icons/library.svg"),
            Self::Search => egui::include_image!("../../assets/icons/search.svg"),
            Self::Settings => egui::include_image!("../../assets/icons/settings.svg"),
            Self::Open => egui::include_image!("../../assets/icons/folder-open.svg"),
            Self::More => egui::include_image!("../../assets/icons/more.svg"),
            Self::Close => egui::include_image!("../../assets/icons/x.svg"),
            Self::ChevronLeft => egui::include_image!("../../assets/icons/chevron-left.svg"),
            Self::ChevronRight => egui::include_image!("../../assets/icons/chevron-right.svg"),
            Self::ZoomIn => egui::include_image!("../../assets/icons/zoom-in.svg"),
            Self::ZoomOut => egui::include_image!("../../assets/icons/zoom-out.svg"),
            Self::Rotate => egui::include_image!("../../assets/icons/rotate.svg"),
            Self::Bookmark => egui::include_image!("../../assets/icons/bookmark.svg"),
            Self::Highlight => egui::include_image!("../../assets/icons/highlighter.svg"),
            Self::Note => egui::include_image!("../../assets/icons/note.svg"),
            Self::Lock => egui::include_image!("../../assets/icons/lock.svg"),
        }
    }

    pub fn image(self, size: f32) -> Image<'static> {
        Image::new(self.source()).fit_to_exact_size(Vec2::splat(size))
    }
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> Response {
    ui.add_sized([36.0, 36.0], egui::Button::image(icon.image(18.0)).frame(false).image_tint_follows_text_color(true))
        .on_hover_text(tooltip)
}

pub fn icon_text_button(ui: &mut egui::Ui, icon: Icon, text: &str) -> Response {
    ui.add_sized([ui.available_width(), 36.0], egui::Button::image_and_text(icon.image(17.0), text).image_tint_follows_text_color(true))
}

pub fn navigation_button(ui: &mut egui::Ui, icon: Icon, text: &str, selected: bool) -> Response {
    ui.add_sized(
        [ui.available_width(), 38.0],
        egui::Button::image_and_text(icon.image(17.0), text).selected(selected).image_tint_follows_text_color(true),
    )
}

pub fn configure(ctx: &egui::Context, theme: AppTheme, reduced_motion: bool) {
    let p = Palette::for_theme(theme);
    let dark = theme.is_dark();
    let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.override_text_color = Some(p.text);
    visuals.weak_text_color = Some(p.muted);
    visuals.panel_fill = p.app_bg;
    visuals.window_fill = p.elevated;
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.extreme_bg_color = p.elevated;
    visuals.text_edit_bg_color = Some(p.elevated);
    visuals.faint_bg_color = p.accent_soft;
    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.widgets.noninteractive.bg_fill = p.panel;
    visuals.widgets.noninteractive.weak_bg_fill = p.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.inactive.bg_fill = p.elevated;
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.hovered.bg_fill = p.accent_soft;
    visuals.widgets.hovered.weak_bg_fill = p.accent_soft;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.3, p.accent);
    visuals.widgets.active.bg_fill = p.accent_soft;
    visuals.widgets.active.weak_bg_fill = p.accent_soft;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, p.accent);
    for widget in [&mut visuals.widgets.noninteractive, &mut visuals.widgets.inactive, &mut visuals.widgets.hovered, &mut visuals.widgets.active, &mut visuals.widgets.open] {
        widget.corner_radius = CornerRadius::same(CONTROL_RADIUS);
    }
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.slider_trailing_fill = true;
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.animation_time = if reduced_motion { 0.0 } else { MOTION_FAST };
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 6.0);
        style.spacing.interact_size = Vec2::new(36.0, 36.0);
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.text_styles.insert(TextStyle::Body, ui_font(14.0));
        style.text_styles.insert(TextStyle::Button, ui_font(13.0));
        style.text_styles.insert(TextStyle::Small, ui_font(12.0));
        style.text_styles.insert(TextStyle::Heading, display_font(22.0));
    });
}

#[cfg(test)]
fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let linear = |value: u8| { let value = value as f32 / 255.0; if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) } };
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    }
    let (bright, dark) = { let aa = luminance(a); let bb = luminance(b); if aa > bb { (aa, bb) } else { (bb, aa) } };
    (bright + 0.05) / (dark + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_breakpoints_are_stable() {
        assert_eq!(LayoutClass::for_width(640.0), LayoutClass::Small);
        assert_eq!(LayoutClass::for_width(900.0), LayoutClass::Medium);
        assert_eq!(LayoutClass::for_width(1400.0), LayoutClass::Large);
    }

    #[test]
    fn tabs_and_library_cards_stay_within_editorial_component_bounds() {
        assert!((140.0..=240.0).contains(&TAB_WIDTH));
        assert!(SIDEBAR_MIN_WIDTH <= SIDEBAR_WIDTH && SIDEBAR_WIDTH <= SIDEBAR_MAX_WIDTH);
        assert!(SIDEBAR_MAX_WIDTH <= 420.0);
        assert_eq!(LIBRARY_CARD_WIDTH, 184.0);
        assert_eq!(LIBRARY_CARD_HEIGHT, 354.0);
    }

    #[test]
    fn primary_text_meets_wcag_contrast() {
        for theme in AppTheme::ALL {
            let p = Palette::for_theme(theme);
            assert!(contrast_ratio(p.text, p.panel) >= 4.5);
            assert!(contrast_ratio(p.muted, p.panel) >= 4.5);
            assert!(contrast_ratio(p.accent, p.panel) >= 3.0);
            assert!(contrast_ratio(p.danger, p.panel) >= 3.0);
        }
    }

    #[test]
    fn white_and_amoled_use_true_extreme_surfaces() {
        let white = Palette::for_theme(AppTheme::White);
        assert_eq!((white.app_bg, white.panel, white.elevated, white.canvas), (Color32::WHITE, Color32::WHITE, Color32::WHITE, Color32::WHITE));

        let amoled = Palette::for_theme(AppTheme::Amoled);
        assert_eq!((amoled.app_bg, amoled.panel, amoled.elevated, amoled.canvas), (Color32::BLACK, Color32::BLACK, Color32::BLACK, Color32::BLACK));
        assert!(AppTheme::Amoled.is_dark());
        assert!(!AppTheme::White.is_dark());
    }

    #[test]
    fn palette_interpolation_keeps_exact_endpoints() {
        let light = Palette::for_theme(AppTheme::Light);
        let dark = Palette::for_theme(AppTheme::Dark);
        assert_eq!(light.blend(dark, 0.0).app_bg, light.app_bg);
        assert_eq!(light.blend(dark, 1.0).app_bg, dark.app_bg);
        assert_eq!(light.blend(dark, -1.0).text, light.text);
        assert_eq!(light.blend(dark, 2.0).text, dark.text);
    }

    #[test]
    fn reduced_motion_disables_builtin_transitions() {
        let ctx = egui::Context::default();
        configure(&ctx, AppTheme::Light, true);
        assert_eq!(ctx.style_of(egui::Theme::Light).animation_time, 0.0);
        configure(&ctx, AppTheme::Light, false);
        assert_eq!(ctx.style_of(egui::Theme::Light).animation_time, MOTION_FAST);
    }

    #[test]
    fn status_bar_has_room_for_small_text_and_frame_padding() {
        let small_text_line_height = 12.0 * 1.4;
        let required = small_text_line_height + f32::from(STATUS_VERTICAL_PADDING) * 2.0 + 2.0;
        assert!(STATUS_HEIGHT >= required);
    }
}
