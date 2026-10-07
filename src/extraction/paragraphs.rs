use crate::document::{Block, BlockKind, RawGlyph, RawPage, SourceRange};

#[derive(Clone)]
struct VisualLine {
    glyphs: Vec<RawGlyph>,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    font_size: f32,
}

impl VisualLine {
    fn text(&self) -> String { self.glyphs.iter().map(|glyph| glyph.ch).collect::<String>().trim().to_owned() }
}

/// Geometry-aware reconstruction used by the production reflow path.
pub fn blocks_from_raw_page(page: &RawPage, first_id: u64) -> Vec<Block> {
    let mut glyphs: Vec<_> = page.glyphs.iter().filter(|glyph| !glyph.ch.is_control()).cloned().collect();
    glyphs.sort_by(|a, b| b.bounds.top.total_cmp(&a.bounds.top).then(a.bounds.left.total_cmp(&b.bounds.left)));
    let mut lines: Vec<VisualLine> = Vec::new();
    for glyph in glyphs {
        let center = (glyph.bounds.top + glyph.bounds.bottom) * 0.5;
        let line = lines.iter_mut().find(|line| {
            let line_center = (line.top + line.bottom) * 0.5;
            (line_center - center).abs() <= glyph.font_size.max(line.font_size) * 0.45
        });
        if let Some(line) = line {
            line.left = line.left.min(glyph.bounds.left);
            line.right = line.right.max(glyph.bounds.right);
            line.top = line.top.max(glyph.bounds.top);
            line.bottom = line.bottom.min(glyph.bounds.bottom);
            line.font_size = line.font_size.max(glyph.font_size);
            line.glyphs.push(glyph);
        } else {
            lines.push(VisualLine { left: glyph.bounds.left, right: glyph.bounds.right, top: glyph.bounds.top, bottom: glyph.bounds.bottom, font_size: glyph.font_size, glyphs: vec![glyph] });
        }
    }
    let mut split_lines = Vec::new();
    for mut line in lines {
        line.glyphs.sort_by(|a, b| a.bounds.left.total_cmp(&b.bounds.left));
        let mut group = Vec::new();
        let mut previous_right = None;
        for glyph in line.glyphs {
            if previous_right.is_some_and(|right| glyph.bounds.left - right > page.width * 0.12) && !group.is_empty() {
                split_lines.push(line_from_glyphs(std::mem::take(&mut group)));
            }
            previous_right = Some(glyph.bounds.right);
            group.push(glyph);
        }
        if !group.is_empty() { split_lines.push(line_from_glyphs(group)); }
    }
    let mut lines = split_lines;
    lines.retain(|line| !line.text().is_empty());

    let midpoint = page.width * 0.5;
    let left_count = lines.iter().filter(|line| line.right < midpoint * 1.08).count();
    let right_count = lines.iter().filter(|line| line.left > midpoint * 0.92).count();
    let two_columns = left_count >= 3 && right_count >= 3;
    lines.sort_by(|a, b| {
        let column = |line: &VisualLine| if two_columns && line.left > midpoint * 0.92 { 1 } else { 0 };
        column(a).cmp(&column(b)).then(b.top.total_cmp(&a.top)).then(a.left.total_cmp(&b.left))
    });

    let median_font = {
        let mut sizes: Vec<_> = lines.iter().map(|line| line.font_size).collect();
        sizes.sort_by(f32::total_cmp);
        sizes.get(sizes.len() / 2).copied().unwrap_or(12.0)
    };
    let mut blocks = Vec::new();
    let mut current: Vec<VisualLine> = Vec::new();
    for line in lines {
        let heading = line.font_size > median_font * 1.18 || looks_like_heading(&line.text());
        let gap = current.last().map(|previous| previous.bottom - line.top).unwrap_or(0.0).abs();
        let column_break = current.last().is_some_and(|previous| two_columns && (previous.left > midpoint) != (line.left > midpoint));
        if heading || column_break || (!current.is_empty() && gap > median_font * 1.15) {
            flush_lines(page.page, first_id, &mut blocks, &mut current, median_font);
        }
        current.push(line);
        if heading { flush_lines(page.page, first_id, &mut blocks, &mut current, median_font); }
    }
    flush_lines(page.page, first_id, &mut blocks, &mut current, median_font);
    blocks
}

fn line_from_glyphs(mut glyphs: Vec<RawGlyph>) -> VisualLine {
    // Geometry tells us which glyphs share a visual line, but it is not a
    // trustworthy character-order signal. PDF glyph boxes routinely overlap
    // or give spaces surprising coordinates because of kerning and embedded
    // font metrics. PDFium's native character index preserves the content
    // stream's text order, so restore that order after spatial grouping.
    glyphs.sort_by_key(|glyph| glyph.source.char_index);
    VisualLine {
        left: glyphs.iter().map(|glyph| glyph.bounds.left).fold(f32::INFINITY, f32::min),
        right: glyphs.iter().map(|glyph| glyph.bounds.right).fold(f32::NEG_INFINITY, f32::max),
        top: glyphs.iter().map(|glyph| glyph.bounds.top).fold(f32::NEG_INFINITY, f32::max),
        bottom: glyphs.iter().map(|glyph| glyph.bounds.bottom).fold(f32::INFINITY, f32::min),
        font_size: glyphs.iter().map(|glyph| glyph.font_size).fold(0.0, f32::max),
        glyphs,
    }
}

fn flush_lines(page: u32, first_id: u64, blocks: &mut Vec<Block>, lines: &mut Vec<VisualLine>, median_font: f32) {
    if lines.is_empty() { return; }
    let mut content = String::new();
    for line in lines.iter() {
        let text = line.text();
        if content.ends_with('-') && starts_lowercase(&text) { content.pop(); } else if !content.is_empty() { content.push(' '); }
        content.push_str(&text);
    }
    let start = lines.iter().flat_map(|line| &line.glyphs).map(|glyph| glyph.source.char_index).min().unwrap_or(0);
    let end = lines.iter().flat_map(|line| &line.glyphs).map(|glyph| glyph.source.char_index).max().unwrap_or(start).saturating_add(1);
    let heading = lines.len() == 1 && (lines[0].font_size > median_font * 1.18 || looks_like_heading(&content));
    let kind = if heading { BlockKind::Heading { level: if lines[0].font_size > median_font * 1.45 { 1 } else { 2 } } } else { classify(&content) };
    blocks.push(Block { id: first_id + blocks.len() as u64, kind, content, source: vec![SourceRange { page, start_char: start, end_char: end }], confidence: if lines.len() > 1 { 0.9 } else { 0.82 } });
    lines.clear();
}

/// Converts PDF text into semantic blocks. This is deliberately pure so more
/// sophisticated geometry-based reading order can replace it independently.
pub fn blocks_from_page_text(page: u32, text: &str, first_id: u64) -> Vec<Block> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let mut start = 0usize;
    let mut cursor = 0usize;

    let flush = |blocks: &mut Vec<Block>, paragraph: &mut String, start: usize, end: usize| {
        let content = paragraph.trim();
        if content.is_empty() {
            paragraph.clear();
            return;
        }
        let kind = classify(content);
        blocks.push(Block {
            id: first_id + blocks.len() as u64,
            kind,
            content: content.to_owned(),
            source: vec![SourceRange {
                page,
                start_char: start as u32,
                end_char: end as u32,
            }],
            confidence: 0.82,
        });
        paragraph.clear();
    };

    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut blocks, &mut paragraph, start, cursor);
            cursor += line.len() + 1;
            start = cursor;
            continue;
        }

        if paragraph.is_empty() {
            start = cursor;
        } else if paragraph.ends_with('-') && starts_lowercase(trimmed) {
            paragraph.pop();
        } else {
            paragraph.push(' ');
        }
        paragraph.push_str(trimmed);

        if ends_sentence(trimmed) || looks_like_heading(trimmed) {
            flush(&mut blocks, &mut paragraph, start, cursor + line.len());
        }
        cursor += line.len() + 1;
    }
    flush(&mut blocks, &mut paragraph, start, normalized.len());
    blocks
}

fn classify(text: &str) -> BlockKind {
    if looks_like_heading(text) {
        BlockKind::Heading { level: if text.len() < 40 { 1 } else { 2 } }
    } else if text.starts_with("• ") || text.starts_with("- ") || text.starts_with("– ") {
        BlockKind::ListItem
    } else {
        BlockKind::Paragraph
    }
}

fn starts_lowercase(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_lowercase)
}

fn ends_sentence(text: &str) -> bool {
    text.ends_with(['.', '!', '?', '”', '’']) && text.len() > 35
}

fn looks_like_heading(text: &str) -> bool {
    let letters: Vec<_> = text.chars().filter(|c| c.is_alphabetic()).collect();
    let words: Vec<_> = text.split_whitespace().collect();
    let title_case = !words.is_empty()
        && words.len() <= 8
        && words.iter().all(|word| {
            word.chars()
                .find(|c| c.is_alphabetic())
                .is_none_or(char::is_uppercase)
        });
    !letters.is_empty()
        && text.len() < 90
        && !text.ends_with(['.', ',', ';', '-'])
        && (title_case
            || letters.iter().all(|c| c.is_uppercase())
            || text.to_ascii_lowercase().starts_with("chapter "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{PageRect, RawGlyph, RawPage, SourcePosition};

    #[test]
    fn joins_visual_lines_and_repairs_soft_hyphenation() {
        let input = "CHAPTER ONE\n\nA long para-\ngraph continues on the next line and ends here.\n";
        let blocks = blocks_from_page_text(2, input, 0);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0].kind, BlockKind::Heading { .. }));
        assert_eq!(blocks[1].content, "A long paragraph continues on the next line and ends here.");
        assert_eq!(blocks[1].source[0].page, 2);
    }

    #[test]
    fn geometry_orders_two_columns_before_crossing_to_the_right() {
        let mut glyphs = Vec::new();
        let mut index = 0;
        for (x, prefix) in [(40.0, "Left"), (340.0, "Right")] {
            for (line, suffix) in ["one.", "two.", "three."].into_iter().enumerate() {
                for (offset, ch) in format!("{prefix} {suffix}").chars().enumerate() {
                    glyphs.push(RawGlyph { source: SourcePosition { page: 0, char_index: index }, ch, bounds: PageRect { left: x + offset as f32 * 7.0, right: x + offset as f32 * 7.0 + 6.0, top: 760.0 - line as f32 * 20.0, bottom: 750.0 - line as f32 * 20.0 }, font_name: "Test".into(), font_size: 12.0 });
                    index += 1;
                }
            }
        }
        let page = RawPage { page: 0, width: 600.0, height: 800.0, glyphs, text: String::new(), links: vec![] };
        let blocks = blocks_from_raw_page(&page, 1);
        assert!(blocks[0].content.starts_with("Left"));
        assert!(blocks.last().unwrap().content.starts_with("Right"));
        assert!(blocks[0].source[0].end_char > blocks[0].source[0].start_char);
    }

    #[test]
    fn geometry_does_not_scramble_native_character_order() {
        let text = "Email Policy";
        let glyphs = text.chars().enumerate().map(|(index, ch)| {
            // Deliberately make several neighboring glyph boxes disagree with
            // the logical character order, as happens with embedded PDF fonts.
            let visual_slot = match index { 1 => 2, 2 => 1, 5 => 6, 6 => 5, _ => index };
            RawGlyph {
                source: SourcePosition { page: 0, char_index: index as u32 },
                ch,
                bounds: PageRect {
                    left: 40.0 + visual_slot as f32 * 7.0,
                    right: 46.0 + visual_slot as f32 * 7.0,
                    top: 760.0,
                    bottom: 748.0,
                },
                font_name: "Embedded".into(),
                font_size: 12.0,
            }
        }).collect();
        let page = RawPage { page: 0, width: 600.0, height: 800.0, glyphs, text: text.into(), links: vec![] };

        let blocks = blocks_from_raw_page(&page, 1);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, text);
    }
}
