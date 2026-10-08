use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadingMode {
    Original,
    Reflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderTheme {
    Paper,
    Sepia,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppTheme {
    Light,
    Dark,
    White,
    OffWhite,
    Sepia,
    Gray,
    Amoled,
}

impl AppTheme {
    pub const ALL: [Self; 7] = [Self::Light, Self::Dark, Self::White, Self::OffWhite, Self::Sepia, Self::Gray, Self::Amoled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Forest Light",
            Self::Dark => "Forest Dark",
            Self::White => "White",
            Self::OffWhite => "Off White",
            Self::Sepia => "Sepia",
            Self::Gray => "Gray",
            Self::Amoled => "AMOLED",
        }
    }

    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark | Self::Amoled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlignment {
    Left,
    Center,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderStyle {
    pub app_theme: AppTheme,
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub paragraph_spacing: f32,
    pub content_max_width: f32,
    pub horizontal_margin: f32,
    pub alignment: TextAlignment,
    pub theme: ReaderTheme,
    pub reduced_motion: bool,
}

impl Default for ReaderStyle {
    fn default() -> Self {
        Self {
            app_theme: AppTheme::Light,
            font_family: "System serif".into(),
            font_size: 18.0,
            line_height: 1.6,
            letter_spacing: 0.0,
            paragraph_spacing: 18.0,
            content_max_width: 680.0,
            horizontal_margin: 32.0,
            alignment: TextAlignment::Left,
            theme: ReaderTheme::Paper,
            reduced_motion: false,
        }
    }
}

impl ReaderStyle {
    pub(crate) fn sanitize(&mut self) {
        let defaults = Self::default();
        if !matches!(self.font_family.as_str(), "System serif" | "System sans-serif" | "Monospace") {
            self.font_family = defaults.font_family;
        }
        self.font_size = finite_clamp(self.font_size, defaults.font_size, 12.0, 40.0);
        self.line_height = finite_clamp(self.line_height, defaults.line_height, 1.1, 2.2);
        self.letter_spacing = finite_clamp(self.letter_spacing, defaults.letter_spacing, -0.5, 3.0);
        self.paragraph_spacing = finite_clamp(self.paragraph_spacing, defaults.paragraph_spacing, 6.0, 36.0);
        self.content_max_width = finite_clamp(self.content_max_width, defaults.content_max_width, 440.0, 960.0);
        self.horizontal_margin = finite_clamp(self.horizontal_margin, defaults.horizontal_margin, 16.0, 80.0);
    }
}

fn finite_clamp(value: f32, fallback: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() { value.clamp(minimum, maximum) } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_style_values_are_bounded_before_rendering() {
        let mut style = ReaderStyle {
            font_family: "Missing font".into(),
            font_size: f32::NAN,
            line_height: -50.0,
            letter_spacing: 90.0,
            paragraph_spacing: f32::INFINITY,
            content_max_width: 1.0,
            horizontal_margin: 10_000.0,
            ..ReaderStyle::default()
        };
        style.sanitize();
        assert_eq!(style.font_family, "System serif");
        assert_eq!(style.font_size, 18.0);
        assert_eq!(style.line_height, 1.1);
        assert_eq!(style.letter_spacing, 3.0);
        assert_eq!(style.paragraph_spacing, 18.0);
        assert_eq!(style.content_max_width, 440.0);
        assert_eq!(style.horizontal_margin, 80.0);
    }

    #[test]
    fn every_application_theme_round_trips_through_settings_json() {
        for theme in AppTheme::ALL {
            let style = ReaderStyle { app_theme: theme, ..ReaderStyle::default() };
            let json = serde_json::to_string(&style).unwrap();
            let restored: ReaderStyle = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.app_theme, theme);
            assert!(!theme.label().is_empty());
        }
    }
}
