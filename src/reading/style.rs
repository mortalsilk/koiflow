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
pub enum TextAlignment {
    Left,
    Center,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderStyle {
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub paragraph_spacing: f32,
    pub content_max_width: f32,
    pub horizontal_margin: f32,
    pub alignment: TextAlignment,
    pub theme: ReaderTheme,
}

impl Default for ReaderStyle {
    fn default() -> Self {
        Self {
            font_family: "System serif".into(),
            font_size: 18.0,
            line_height: 1.6,
            letter_spacing: 0.0,
            paragraph_spacing: 18.0,
            content_max_width: 680.0,
            horizontal_margin: 32.0,
            alignment: TextAlignment::Left,
            theme: ReaderTheme::Paper,
        }
    }
}
