//! Lightweight Markdown and code-syntax renderer for the chat transcript.
//!
//! No external crates: covers headings, paragraphs, bold/italic/inline code,
//! links, bullet and numbered lists, task checkboxes (`- [ ]` / `- [x]`),
//! blockquotes, horizontal rules, pipe tables, fenced code blocks with syntax
//! highlighting, and unified diffs.

use crate::settings::{ThemeName, ThemePalette};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Render a Markdown string into styled, width-wrapped lines using the default Dark palette.
#[allow(dead_code)]
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    render_with_theme(text, width, &ThemeName::Dark.palette())
}

/// Render a Markdown string into styled, width-wrapped lines using `palette`.
pub fn render_with_theme(
    text: &str,
    width: usize,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let base = Style::default().fg(palette.fg);

    for block in parse(text) {
        match block {
            Block::Blank => lines.push(Line::from(Span::raw(""))),
            Block::Code { lang, source } => lines.extend(render_code(&lang, &source, width, palette)),
            Block::Heading { level, text } => {
                lines.extend(render_heading(level, &text, width, palette))
            }
            Block::Rule => lines.push(rule_line(width, palette)),
            Block::Checkbox { checked, text } => {
                let (prefix, p_style) = if checked {
                    ("☑ ", Style::default().fg(palette.success).add_modifier(Modifier::BOLD))
                } else {
                    ("☐ ", Style::default().fg(palette.muted))
                };
                let t_style = if checked {
                    Style::default().fg(palette.muted)
                } else {
                    base
                };
                lines.extend(render_prefixed(
                    &text,
                    width,
                    prefix,
                    "  ",
                    t_style,
                    p_style,
                    palette,
                ));
            }
            Block::Bullet(text) => lines.extend(render_prefixed(
                &text,
                width,
                "• ",
                "  ",
                base,
                Style::default().fg(palette.bullet),
                palette,
            )),
            Block::Ordered { number, text } => {
                let marker = format!("{number}. ");
                let cont = " ".repeat(UnicodeWidthStr::width(marker.as_str()));
                lines.extend(render_prefixed(
                    &text,
                    width,
                    &marker,
                    &cont,
                    base,
                    Style::default().fg(palette.bullet),
                    palette,
                ));
            }
            Block::Quote(text) => lines.extend(render_prefixed(
                &text,
                width,
                "│ ",
                "│ ",
                Style::default()
                    .fg(palette.muted)
                    .add_modifier(Modifier::ITALIC),
                Style::default().fg(palette.accent_alt),
                palette,
            )),
            Block::Table { headers, rows } => {
                lines.extend(render_table(&headers, &rows, width, palette));
            }
            Block::Paragraph(text) => {
                let runs = inline(&text, base, palette);
                lines.extend(wrap_runs(runs, width));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::raw("")));
    }
    lines
}

/// Render a unified diff snippet into colored, width-wrapped lines.
pub fn render_diff(diff: &str, width: usize, palette: &ThemePalette) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw in diff.lines() {
        let style = diff_line_style(raw, palette);
        let text = if raw.is_empty() { " " } else { raw };
        let mut rest = text;
        while !rest.is_empty() {
            let (chunk, remainder) = split_at_width(rest, width);
            lines.push(Line::from(Span::styled(pad_right(&chunk, width), style)));
            rest = remainder;
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(brak zmian)".to_string(),
            Style::default().fg(palette.muted),
        )));
    }
    lines
}

/// Recover the plain text of styled lines (used for selection/copy).
pub fn plain_lines(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

// --- block model ---------------------------------------------------------

enum Block {
    Blank,
    Code { lang: String, source: String },
    Heading { level: u8, text: String },
    Rule,
    Checkbox { checked: bool, text: String },
    Bullet(String),
    Ordered { number: u64, text: String },
    Quote(String),
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    Paragraph(String),
}

fn parse(text: &str) -> Vec<Block> {
    let raw: Vec<&str> = text.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut i = 0usize;

    while i < raw.len() {
        let line = raw[i];

        if let Some(lang) = fence_open(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            let mut source: Vec<&str> = Vec::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < raw.len() {
                if fence_close(raw[j]) {
                    closed = true;
                    break;
                }
                source.push(raw[j]);
                j += 1;
            }
            blocks.push(Block::Code {
                lang,
                source: source.join("\n"),
            });
            i = if closed { j + 1 } else { j };
            continue;
        }

        if line.trim().is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph);
            if !matches!(blocks.last(), Some(Block::Blank)) {
                blocks.push(Block::Blank);
            }
            i += 1;
            continue;
        }

        if let Some((level, rest)) = heading(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Heading {
                level,
                text: rest,
            });
            i += 1;
            continue;
        }

        if is_rule(line.trim()) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }

        if i + 1 < raw.len() && is_table_row(line) && is_table_separator(raw[i + 1]) {
            flush_paragraph(&mut blocks, &mut paragraph);
            let headers = split_table_cells(line);
            let mut rows = Vec::new();
            let mut j = i + 2;
            while j < raw.len() && is_table_row(raw[j]) && !raw[j].trim().is_empty() {
                rows.push(split_table_cells(raw[j]));
                j += 1;
            }
            blocks.push(Block::Table { headers, rows });
            i = j;
            continue;
        }

        if let Some(rest) = strip_quote(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Quote(rest.to_string()));
            i += 1;
            continue;
        }

        if let Some((checked, rest)) = strip_checkbox(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Checkbox {
                checked,
                text: rest.to_string(),
            });
            i += 1;
            continue;
        }

        if let Some(rest) = strip_bullet(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Bullet(rest.to_string()));
            i += 1;
            continue;
        }

        if let Some((number, rest)) = strip_ordered(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Ordered {
                number,
                text: rest.to_string(),
            });
            i += 1;
            continue;
        }

        paragraph.push(line.trim());
        i += 1;
    }
    flush_paragraph(&mut blocks, &mut paragraph);
    blocks
}

fn flush_paragraph(blocks: &mut Vec<Block>, paragraph: &mut Vec<&str>) {
    if !paragraph.is_empty() {
        blocks.push(Block::Paragraph(paragraph.join(" ")));
        paragraph.clear();
    }
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|&c| c == first).count();
    if count >= 3 {
        Some((first, count))
    } else {
        None
    }
}

fn fence_open(line: &str) -> Option<String> {
    let (_, count) = fence_marker(line)?;
    let trimmed = line.trim();
    Some(trimmed[count..].trim().to_string())
}

fn fence_close(line: &str) -> bool {
    let Some((_, count)) = fence_marker(line) else {
        return false;
    };
    line.trim()[count..].trim().is_empty()
}

fn heading(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start();
    let count = trimmed.chars().take_while(|&c| c == '#').count();
    if count == 0 || count > 6 {
        return None;
    }
    let rest = &trimmed[count..];
    if rest.is_empty() {
        Some((count as u8, String::new()))
    } else if rest.starts_with(' ') {
        Some((count as u8, rest.trim_start().to_string()))
    } else {
        None
    }
}

fn is_rule(text: &str) -> bool {
    if text.len() < 3 {
        return false;
    }
    let marker = text.chars().next().unwrap();
    if marker != '-' && marker != '*' && marker != '_' {
        return false;
    }
    text.chars().all(|c| c == marker || c == ' ')
}

fn strip_quote(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn strip_checkbox(line: &str) -> Option<(bool, &str)> {
    let rest = strip_bullet(line)?;
    if let Some(body) = rest.strip_prefix("[ ] ") {
        Some((false, body))
    } else if let Some(body) = rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
        Some((true, body))
    } else {
        None
    }
}

fn strip_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn strip_ordered(line: &str) -> Option<(u64, &str)> {
    let trimmed = line.trim_start();
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &trimmed[digits.len()..];
    let text = rest.strip_prefix(". ")?;
    Some((digits.parse().unwrap_or(1), text))
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && trimmed.len() >= 3
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') || !trimmed.contains('-') {
        return false;
    }
    let cells = split_table_cells(trimmed);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let c = cell.trim();
            !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
        })
}

fn split_table_cells(line: &str) -> Vec<String> {
    let mut trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix('|') {
        trimmed = rest;
    }
    if let Some(rest) = trimmed.strip_suffix('|') {
        trimmed = rest;
    }
    trimmed
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

// --- block renderers -----------------------------------------------------

fn render_heading(
    level: u8,
    text: &str,
    width: usize,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let mut style = Style::default()
        .fg(palette.heading)
        .add_modifier(Modifier::BOLD);
    if level <= 2 {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    let mut lines = wrap_runs(inline(text, style, palette), width);
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("")));
    }
    lines
}

fn render_prefixed(
    text: &str,
    width: usize,
    first: &str,
    cont: &str,
    text_style: Style,
    prefix_style: Style,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let runs = inline(text, text_style, palette);
    let wrap_width = width.saturating_sub(UnicodeWidthStr::width(cont)).max(1);
    let mut lines = wrap_runs(runs, wrap_width);
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("")));
    }
    for (index, line) in lines.iter_mut().enumerate() {
        let prefix = if index == 0 { first } else { cont };
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled(prefix.to_string(), prefix_style));
        spans.extend(line.spans.iter().cloned());
        *line = Line::from(spans);
    }
    lines
}

fn render_table(
    headers: &[String],
    rows: &[Vec<String>],
    width: usize,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let cols = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return Vec::new();
    }

    let mut col_widths = vec![1usize; cols];
    for (c, h) in headers.iter().enumerate() {
        col_widths[c] = col_widths[c].max(UnicodeWidthStr::width(h.as_str()));
    }
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            col_widths[c] = col_widths[c].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }

    // Shrink widest columns iteratively if table exceeds available width.
    let separator_overhead = cols.saturating_sub(1) * 3 + 2; // " │ " between cols + outer spaces
    let max_content = width.saturating_sub(separator_overhead).max(cols * 3);
    while col_widths.iter().sum::<usize>() > max_content {
        let Some((idx, _)) = col_widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
        else {
            break;
        };
        if col_widths[idx] <= 3 {
            break;
        }
        col_widths[idx] -= 1;
    }

    let border_style = Style::default().fg(palette.muted);
    let header_style = Style::default()
        .fg(palette.heading)
        .add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(palette.fg);

    let format_row = |cells: &[String], style: Style| -> Line<'static> {
        let mut spans = Vec::new();
        spans.push(Span::styled(" ".to_string(), style));
        for (c, col_w) in col_widths.iter().enumerate() {
            if c > 0 {
                spans.push(Span::styled(" │ ".to_string(), border_style));
            }
            let text = cells.get(c).map(String::as_str).unwrap_or("");
            let fitted = fit_cell(text, *col_w);
            spans.push(Span::styled(fitted, style));
        }
        Line::from(spans)
    };

    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(format_row(headers, header_style));

    let mut sep_spans = vec![Span::styled("─".to_string(), border_style)];
    for (c, col_w) in col_widths.iter().enumerate() {
        if c > 0 {
            sep_spans.push(Span::styled("─┼─".to_string(), border_style));
        }
        sep_spans.push(Span::styled("─".repeat(*col_w), border_style));
    }
    sep_spans.push(Span::styled("─".to_string(), border_style));
    out.push(Line::from(sep_spans));

    for row in rows {
        out.push(format_row(row, cell_style));
    }
    out
}

fn fit_cell(text: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(text);
    if w <= width {
        return format!("{text}{}", " ".repeat(width - w));
    }
    if width <= 1 {
        let (chunk, _) = split_at_width(text, width);
        return chunk;
    }
    let (chunk, _) = split_at_width(text, width - 1);
    let trimmed_w = UnicodeWidthStr::width(chunk.as_str());
    let pad = width.saturating_sub(trimmed_w + 1);
    format!("{chunk}…{}", " ".repeat(pad))
}

fn render_code(
    lang: &str,
    source: &str,
    width: usize,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let body = Style::default().fg(palette.code_fg).bg(palette.code_bg);
    let header_style = Style::default().fg(palette.muted).bg(palette.code_bg);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let normalized_lang = lang.trim().to_ascii_lowercase();

    if !normalized_lang.is_empty() {
        let label = format!(" ╭─ {} ", normalized_lang);
        let label_w = UnicodeWidthStr::width(label.as_str());
        let dashes = "─".repeat(width.saturating_sub(label_w));
        lines.push(Line::from(Span::styled(
            format!("{label}{dashes}"),
            header_style,
        )));
    }

    if normalized_lang == "diff" || normalized_lang == "patch" {
        lines.extend(render_diff(source, width, palette));
        return lines;
    }

    let profile = LangProfile::from_lang(&normalized_lang);
    for raw in source.lines() {
        let text = if raw.is_empty() { " " } else { raw };
        let tokens = highlight_code_line(text, profile, palette);
        lines.extend(wrap_code_spans(tokens, width, body));
    }
    if source.is_empty() {
        lines.push(Line::from(Span::styled(pad_right(" ", width), body)));
    }
    lines
}

fn rule_line(width: usize, palette: &ThemePalette) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(palette.muted),
    ))
}

// --- syntax highlighting -------------------------------------------------

#[derive(Clone, Copy)]
enum LangProfile {
    CStyle,
    HashStyle,
    Sql,
    Plain,
}

impl LangProfile {
    fn from_lang(lang: &str) -> Self {
        match lang {
            "rust" | "rs" | "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx"
            | "go" | "c" | "cpp" | "c++" | "java" | "kotlin" | "kt" | "swift" | "json"
            | "css" | "scss" => Self::CStyle,
            "python" | "py" | "sh" | "bash" | "zsh" | "shell" | "toml" | "yaml" | "yml"
            | "ruby" | "rb" | "dockerfile" => Self::HashStyle,
            "sql" => Self::Sql,
            _ => Self::Plain,
        }
    }

    fn is_keyword(&self, word: &str) -> bool {
        match self {
            Self::CStyle => matches!(
                word,
                "fn" | "let"
                    | "mut"
                    | "const"
                    | "static"
                    | "pub"
                    | "struct"
                    | "enum"
                    | "impl"
                    | "trait"
                    | "type"
                    | "where"
                    | "match"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "loop"
                    | "in"
                    | "return"
                    | "break"
                    | "continue"
                    | "use"
                    | "mod"
                    | "crate"
                    | "super"
                    | "self"
                    | "Self"
                    | "async"
                    | "await"
                    | "move"
                    | "ref"
                    | "as"
                    | "dyn"
                    | "unsafe"
                    | "function"
                    | "var"
                    | "class"
                    | "import"
                    | "export"
                    | "from"
                    | "default"
                    | "try"
                    | "catch"
                    | "throw"
                    | "new"
                    | "func"
                    | "package"
                    | "defer"
                    | "go"
                    | "select"
                    | "case"
                    | "switch"
                    | "true"
                    | "false"
                    | "null"
                    | "undefined"
                    | "void"
                    | "sizeof"
            ),
            Self::HashStyle => matches!(
                word,
                "def" | "class"
                    | "import"
                    | "from"
                    | "as"
                    | "if"
                    | "elif"
                    | "else"
                    | "for"
                    | "while"
                    | "in"
                    | "not"
                    | "and"
                    | "or"
                    | "is"
                    | "return"
                    | "yield"
                    | "with"
                    | "try"
                    | "except"
                    | "finally"
                    | "raise"
                    | "pass"
                    | "break"
                    | "continue"
                    | "lambda"
                    | "async"
                    | "await"
                    | "True"
                    | "False"
                    | "None"
                    | "true"
                    | "false"
                    | "then"
                    | "fi"
                    | "do"
                    | "done"
                    | "esac"
                    | "local"
                    | "export"
                    | "set"
                    | "function"
                    | "echo"
                    | "exit"
            ),
            Self::Sql => matches!(
                word.to_ascii_uppercase().as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "INSERT"
                    | "INTO"
                    | "VALUES"
                    | "UPDATE"
                    | "SET"
                    | "DELETE"
                    | "CREATE"
                    | "TABLE"
                    | "DROP"
                    | "ALTER"
                    | "JOIN"
                    | "LEFT"
                    | "RIGHT"
                    | "INNER"
                    | "ON"
                    | "GROUP"
                    | "BY"
                    | "ORDER"
                    | "LIMIT"
                    | "AND"
                    | "OR"
                    | "NOT"
                    | "NULL"
                    | "AS"
            ),
            Self::Plain => false,
        }
    }
}

fn highlight_code_line(
    line: &str,
    profile: LangProfile,
    palette: &ThemePalette,
) -> Vec<Span<'static>> {
    let base = Style::default().fg(palette.code_fg).bg(palette.code_bg);
    if matches!(profile, LangProfile::Plain) {
        return vec![Span::styled(line.to_string(), base)];
    }

    let comment_style = Style::default()
        .fg(palette.code_comment)
        .bg(palette.code_bg)
        .add_modifier(Modifier::ITALIC);
    let string_style = Style::default().fg(palette.code_string).bg(palette.code_bg);
    let number_style = Style::default().fg(palette.code_number).bg(palette.code_bg);
    let keyword_style = Style::default()
        .fg(palette.code_keyword)
        .bg(palette.code_bg)
        .add_modifier(Modifier::BOLD);
    let type_style = Style::default().fg(palette.code_type).bg(palette.code_bg);

    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];

        // Comments
        let is_comment = match profile {
            LangProfile::CStyle => {
                ch == '/' && i + 1 < chars.len() && (chars[i + 1] == '/' || chars[i + 1] == '*')
            }
            LangProfile::HashStyle => ch == '#',
            LangProfile::Sql => ch == '-' && i + 1 < chars.len() && chars[i + 1] == '-',
            LangProfile::Plain => false,
        };
        if is_comment {
            push_plain(&mut spans, &mut buf, base);
            let rest: String = chars[i..].iter().collect();
            spans.push(Span::styled(rest, comment_style));
            return spans;
        }

        // String literals ("..." or '...')
        if ch == '"' || ch == '\'' {
            // Do not treat Rust lifetime like `'a` or `'static` as a string literal.
            let is_lifetime = ch == '\''
                && matches!(profile, LangProfile::CStyle)
                && i + 1 < chars.len()
                && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_')
                && !(i + 2 < chars.len() && chars[i + 2] == '\'');
            if !is_lifetime {
                push_plain(&mut spans, &mut buf, base);
                let quote = ch;
                let mut j = i + 1;
                let mut escaped = false;
                while j < chars.len() {
                    if escaped {
                        escaped = false;
                    } else if chars[j] == '\\' {
                        escaped = true;
                    } else if chars[j] == quote {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                let lit: String = chars[i..j].iter().collect();
                spans.push(Span::styled(lit, string_style));
                i = j;
                continue;
            }
        }

        // Numbers
        if ch.is_ascii_digit()
            && (i == 0 || (!chars[i - 1].is_ascii_alphanumeric() && chars[i - 1] != '_'))
        {
            push_plain(&mut spans, &mut buf, base);
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_')
            {
                j += 1;
            }
            let num: String = chars[i..j].iter().collect();
            spans.push(Span::styled(num, number_style));
            i = j;
            continue;
        }

        // Identifiers / keywords / types / macros
        if ch.is_ascii_alphabetic() || ch == '_' {
            push_plain(&mut spans, &mut buf, base);
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let mut word: String = chars[i..j].iter().collect();
            let is_macro = j < chars.len() && chars[j] == '!';
            if is_macro {
                word.push('!');
                j += 1;
            }
            if profile.is_keyword(&word) {
                spans.push(Span::styled(word, keyword_style));
            } else if is_macro
                || word.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            {
                spans.push(Span::styled(word, type_style));
            } else {
                spans.push(Span::styled(word, base));
            }
            i = j;
            continue;
        }

        buf.push(ch);
        i += 1;
    }

    push_plain(&mut spans, &mut buf, base);
    spans
}

fn wrap_code_spans(
    spans: Vec<Span<'static>>,
    width: usize,
    bg_style: Style,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;

    for span in spans {
        let style = span.style;
        let mut rest = span.content.as_ref();
        while !rest.is_empty() {
            let space_left = width.saturating_sub(col);
            if space_left == 0 {
                lines.push(Line::from(std::mem::take(&mut current)));
                col = 0;
                continue;
            }
            let (chunk, remainder) = split_at_width(rest, space_left);
            let chunk_w = UnicodeWidthStr::width(chunk.as_str());
            current.push(Span::styled(chunk, style));
            col += chunk_w;
            rest = remainder;
            if col >= width && !rest.is_empty() {
                lines.push(Line::from(std::mem::take(&mut current)));
                col = 0;
            }
        }
    }

    if col < width {
        current.push(Span::styled(" ".repeat(width - col), bg_style));
    }
    lines.push(Line::from(current));
    lines
}

fn diff_line_style(line: &str, palette: &ThemePalette) -> Style {
    if line.starts_with('+') && !line.starts_with("+++") {
        Style::default().fg(palette.diff_add_fg).bg(palette.diff_add_bg)
    } else if line.starts_with('-') && !line.starts_with("---") {
        Style::default().fg(palette.diff_del_fg).bg(palette.diff_del_bg)
    } else if line.starts_with("@@") {
        Style::default()
            .fg(palette.diff_hunk)
            .bg(palette.code_bg)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("diff ") || line.starts_with("index ") || line.starts_with("+++") || line.starts_with("---") {
        Style::default().fg(palette.muted).bg(palette.code_bg)
    } else {
        Style::default().fg(palette.code_fg).bg(palette.code_bg)
    }
}

// --- inline parsing ------------------------------------------------------

fn inline(text: &str, base: Style, palette: &ThemePalette) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;

    let bold = base.add_modifier(Modifier::BOLD);
    let italic = base.add_modifier(Modifier::ITALIC);
    let code = Style::default()
        .fg(palette.warning)
        .bg(palette.code_bg);
    let link = Style::default()
        .fg(palette.link)
        .add_modifier(Modifier::UNDERLINED);

    while i < chars.len() {
        let ch = chars[i];

        if ch == '*' {
            if i + 1 < chars.len() && chars[i + 1] == '*' {
                if let Some((end, inner)) = find_delim(&chars, i + 2, "**") {
                    push_plain(&mut spans, &mut buf, base);
                    spans.push(Span::styled(inner, bold));
                    i = end;
                    continue;
                }
            } else if let Some((end, inner)) = find_delim(&chars, i + 1, "*") {
                push_plain(&mut spans, &mut buf, base);
                spans.push(Span::styled(inner, italic));
                i = end;
                continue;
            }
        }

        if ch == '`' {
            if let Some((end, inner)) = find_delim(&chars, i + 1, "`") {
                push_plain(&mut spans, &mut buf, base);
                spans.push(Span::styled(inner, code));
                i = end;
                continue;
            }
        }

        if ch == '[' {
            if let Some((end, label)) = find_link(&chars, i) {
                push_plain(&mut spans, &mut buf, base);
                spans.push(Span::styled(label, link));
                i = end;
                continue;
            }
        }

        buf.push(ch);
        i += 1;
    }
    push_plain(&mut spans, &mut buf, base);
    spans
}

fn push_plain(spans: &mut Vec<Span<'static>>, buf: &mut String, base: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), base));
    }
}

fn find_delim(chars: &[char], start: usize, delim: &str) -> Option<(usize, String)> {
    let marker: Vec<char> = delim.chars().collect();
    let mut i = start;
    while i + marker.len() <= chars.len() {
        if chars[i..i + marker.len()] == marker[..] {
            let inner: String = chars[start..i].iter().collect();
            return Some((i + marker.len(), inner));
        }
        i += 1;
    }
    None
}

fn find_link(chars: &[char], start: usize) -> Option<(usize, String)> {
    let mut j = start + 1;
    while j < chars.len() {
        if chars[j] == ']' {
            if j + 1 < chars.len() && chars[j + 1] == '(' {
                let mut k = j + 2;
                while k < chars.len() {
                    if chars[k] == ')' {
                        let label: String = chars[start + 1..j].iter().collect();
                        return Some((k + 1, label));
                    }
                    k += 1;
                }
            }
            return None;
        }
        j += 1;
    }
    None
}

// --- wrapping ------------------------------------------------------------

struct Word {
    text: String,
    style: Style,
}

fn tokenize(runs: Vec<Span<'static>>) -> Vec<Word> {
    let mut words = Vec::new();
    for span in runs {
        let content = span.content.into_owned();
        for word in content.split_whitespace() {
            words.push(Word {
                text: word.to_string(),
                style: span.style,
            });
        }
    }
    words
}

fn wrap_runs(runs: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for word in tokenize(runs) {
        let word_width = UnicodeWidthStr::width(word.text.as_str());
        if word_width == 0 {
            continue;
        }
        let separator = if current.is_empty() { 0usize } else { 1usize };

        if current_width + separator + word_width <= width {
            if separator == 1 {
                current.push(Span::styled(" ".to_string(), word.style));
                current_width += 1;
            }
            current.push(Span::styled(word.text.clone(), word.style));
            current_width += word_width;
        } else if word_width <= width {
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(Span::styled(word.text.clone(), word.style));
            current_width = word_width;
        } else {
            if !current.is_empty() {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
            let mut rest = word.text.as_str();
            while !rest.is_empty() {
                let (chunk, remainder) = split_at_width(rest, width);
                if chunk.is_empty() {
                    break;
                }
                lines.push(Line::from(vec![Span::styled(chunk, word.style)]));
                rest = remainder;
            }
        }
    }

    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

fn split_at_width(text: &str, width: usize) -> (String, &str) {
    let mut columns = 0usize;
    let mut byte = 0usize;
    for (index, ch) in text.char_indices() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if columns + char_width > width {
            break;
        }
        columns += char_width;
        byte = index + ch.len_utf8();
    }
    if byte == 0 {
        let ch = text.chars().next().unwrap();
        (ch.to_string(), &text[ch.len_utf8()..])
    } else {
        (text[..byte].to_string(), &text[byte..])
    }
}

fn pad_right(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    if used >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - used))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_tables_and_checkboxes() {
        let palette = ThemeName::Dark.palette();
        let md = "\
- [ ] Do zrobienia
- [x] Gotowe

| Plik | Status |
|---|---|
| main.rs | OK |
";
        let rendered = render_with_theme(md, 60, &palette);
        let plain = plain_lines(&rendered).join("\n");
        assert!(plain.contains("☐ Do zrobienia"));
        assert!(plain.contains("☑ Gotowe"));
        assert!(plain.contains("main.rs"));
        assert!(plain.contains("│"));
        assert!(plain.contains("─┼─"));
    }

    #[test]
    fn highlights_rust_and_diff_code_blocks() {
        let palette = ThemeName::Sepia.palette();
        let md = "\
```rust
fn main() {
    let x = 42; // comment
    println!(\"hi\");
}
```

```diff
@@ -1,2 +1,2 @@
-old
+new
```
";
        let rendered = render_with_theme(md, 50, &palette);
        let plain = plain_lines(&rendered).join("\n");
        assert!(plain.contains("╭─ rust"));
        assert!(plain.contains("println!(\"hi\")"));
        assert!(plain.contains("+new"));
        assert!(plain.contains("-old"));
        // Verify diff coloring applied distinct foregrounds on +new / -old lines.
        let plus_line = rendered
            .iter()
            .find(|line| line.spans.iter().any(|s| s.content.contains("+new")))
            .unwrap();
        assert_eq!(plus_line.spans[0].style.fg, Some(palette.diff_add_fg));
    }
}
