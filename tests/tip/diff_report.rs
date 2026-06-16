use owo_colors::OwoColorize;
use similar::{Algorithm, ChangeTag, DiffableStr, InlineChange, TextDiff};

const REPORT_WIDTH: usize = 118;

pub(crate) fn assert_text_eq(label: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }

    panic!("{}", text_diff_report(label, expected, actual));
}

pub(crate) fn print_text_diff_report(label: &str, expected: &str, actual: &str) {
    if expected == actual {
        println!("{label}: expected output matched actual output");
        return;
    }

    println!("{}", text_diff_report(label, expected, actual));
}

fn text_diff_report(label: &str, expected: &str, actual: &str) -> String {
    format!(
        "{label} mismatch\n\n{}\n\n{}",
        render_wrapped_text_pipeline_diff(expected, actual),
        render_similar_report(expected, actual)
    )
}

fn render_wrapped_text_pipeline_diff(expected: &str, actual: &str) -> String {
    let mut lines = Vec::new();
    lines.push("┌─ Text Pipeline (steps: 1, changed: 1)".to_string());
    push_wrapped_body(&mut lines, "IN  : ", expected);
    push_wrapped_body(&mut lines, "• expected_vs_actual (applied)", "");
    push_wrapped_body(&mut lines, "  - ", expected);
    push_wrapped_body(&mut lines, "  + ", actual);
    push_wrapped_body(&mut lines, "OUT : ", actual);
    lines.push("└─".to_string());
    lines.join("\n")
}

fn render_similar_report(expected: &str, actual: &str) -> String {
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Patience);
    let diff = config.diff_lines(expected, actual);

    let mut out = String::new();
    out.push_str(&format!(
        "{}\n",
        "similar unified diff (patience)".bold().underline()
    ));
    out.push_str(&colorize_unified_diff(&format!(
        "{}",
        diff.unified_diff()
            .header("expected", "actual")
            .context_radius(3)
    )));
    out.push('\n');
    out.push_str(&format!(
        "\n{}\n",
        "similar inline diff (line numbers + word highlights)"
            .bold()
            .underline()
    ));
    out.push_str(&render_inline_diff(expected, actual));
    out
}

fn render_inline_diff(expected: &str, actual: &str) -> String {
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Patience);
    let diff = config.diff_lines(expected, actual);
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {} │{}│ {}",
        "old".dimmed(),
        "new".dimmed(),
        "±".dimmed(),
        "text".dimmed()
    ));

    for (group_idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if group_idx > 0 {
            lines.push(format!(
                "{}",
                "────────────────────────────────────────────────".dimmed()
            ));
        }
        lines.push(format!(
            "{}",
            format!("@@ group {} @@", group_idx + 1).cyan()
        ));

        for op in group {
            for change in diff.iter_inline_changes(op) {
                lines.push(render_inline_change_line(&change));
            }
        }
    }

    lines.join("\n")
}

fn colorize_unified_diff(diff: &str) -> String {
    diff.lines()
        .flat_map(|line| {
            let wrapped = wrap_chars(line, REPORT_WIDTH);
            if line.starts_with("---") || line.starts_with("+++") {
                styled_lines(wrapped, UnifiedStyle::Header)
            } else if line.starts_with("@@") {
                styled_lines(wrapped, UnifiedStyle::Hunk)
            } else if line.starts_with('+') {
                styled_lines(wrapped, UnifiedStyle::Insert)
            } else if line.starts_with('-') {
                styled_lines(wrapped, UnifiedStyle::Delete)
            } else if line.starts_with("\\ ") {
                styled_lines(wrapped, UnifiedStyle::Context)
            } else {
                styled_lines(wrapped, UnifiedStyle::Context)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn styled_lines(lines: Vec<String>, style: UnifiedStyle) -> Vec<String> {
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let line = if idx == 0 {
                line
            } else {
                format!("      {line}")
            };
            match style {
                UnifiedStyle::Header => line.bold().to_string(),
                UnifiedStyle::Hunk => line.cyan().to_string(),
                UnifiedStyle::Insert => line.green().to_string(),
                UnifiedStyle::Delete => line.red().to_string(),
                UnifiedStyle::Context => line.dimmed().to_string(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum UnifiedStyle {
    Header,
    Hunk,
    Insert,
    Delete,
    Context,
}

fn render_inline_change_line<T>(change: &InlineChange<'_, T>) -> String
where
    T: DiffableStr + ?Sized,
{
    let sign = match change.tag() {
        ChangeTag::Delete => "-",
        ChangeTag::Insert => "+",
        ChangeTag::Equal => " ",
    };
    let first_prefix = format!(
        "{} {} │{}│",
        line_no(change.old_index()),
        line_no(change.new_index()),
        sign
    );
    let continuation_prefix = format!("{} {} │ │", line_no(None), line_no(None));
    let available_width = REPORT_WIDTH.saturating_sub(visible_len(&first_prefix) + 1);
    let chunks = inline_chunks(change);
    let wrapped_body = wrap_styled_chunks(&chunks, available_width);
    let newline_marker = if change.missing_newline() {
        format!(" {}", "\\ No newline at end of file".dimmed())
    } else {
        String::new()
    };
    let line_count = wrapped_body.len();

    wrapped_body
        .into_iter()
        .enumerate()
        .map(|(idx, body)| {
            let prefix = if idx == 0 {
                &first_prefix
            } else {
                &continuation_prefix
            };
            let marker = if idx + 1 == line_count {
                newline_marker.as_str()
            } else {
                ""
            };

            match change.tag() {
                ChangeTag::Delete => format!("{} {}{}", prefix.red(), body, marker),
                ChangeTag::Insert => format!("{} {}{}", prefix.green(), body, marker),
                ChangeTag::Equal => format!("{} {}{}", prefix.dimmed(), body, marker),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_no(index: Option<usize>) -> String {
    index
        .map(|idx| format!("{:>4}", idx + 1))
        .unwrap_or_else(|| "    ".to_string())
}

fn inline_chunks<T>(change: &InlineChange<'_, T>) -> Vec<StyledText>
where
    T: DiffableStr + ?Sized,
{
    let mut chunks = Vec::new();

    for (emphasized, value) in change.iter_strings_lossy() {
        let escaped = escape_visible(&value);
        let style = match (change.tag(), emphasized) {
            (ChangeTag::Delete, true) => TextStyle::DeleteEmphasis,
            (ChangeTag::Insert, true) => TextStyle::InsertEmphasis,
            (ChangeTag::Delete, false) => TextStyle::Delete,
            (ChangeTag::Insert, false) => TextStyle::Insert,
            (ChangeTag::Equal, _) => TextStyle::Equal,
        };
        chunks.extend(tokenize_styled_text(&escaped, style));
    }

    if matches!(chunks.last(), Some(chunk) if chunk.text == "⏎") {
        chunks.pop();
    }

    chunks
}

fn tokenize_styled_text(value: &str, style: TextStyle) -> Vec<StyledText> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_is_space: Option<bool> = None;

    for ch in value.chars() {
        let is_space = ch.is_whitespace();
        if current_is_space.is_some_and(|kind| kind != is_space) {
            chunks.push(StyledText {
                text: std::mem::take(&mut current),
                style,
            });
        }
        current_is_space = Some(is_space);
        current.push(ch);
    }

    if !current.is_empty() {
        chunks.push(StyledText {
            text: current,
            style,
        });
    }

    chunks
}

fn wrap_styled_chunks(chunks: &[StyledText], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for chunk in chunks {
        let mut text = chunk.text.as_str();
        while !text.is_empty() {
            let text_width = visible_len(text);
            if text_width + current_width <= width || current_width == 0 {
                if text_width + current_width <= width {
                    current.push_str(&chunk.render_text(text));
                    current_width += text_width;
                    break;
                }

                let split_at = nth_char_boundary(text, width);
                let (head, tail) = text.split_at(split_at);
                current.push_str(&chunk.render_text(head));
                lines.push(std::mem::take(&mut current));
                current_width = 0;
                text = tail.trim_start();
                continue;
            }

            if !current.trim().is_empty() {
                lines.push(current.trim_end().to_string());
            } else {
                lines.push(std::mem::take(&mut current));
            }
            current.clear();
            current_width = 0;
            text = text.trim_start();
        }
    }

    if !current.is_empty() {
        lines.push(current.trim_end().to_string());
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn nth_char_boundary(value: &str, char_count: usize) -> usize {
    value
        .char_indices()
        .nth(char_count)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len())
}

fn visible_len(value: &str) -> usize {
    value.chars().count()
}

#[derive(Debug, Clone)]
struct StyledText {
    text: String,
    style: TextStyle,
}

impl StyledText {
    fn render_text(&self, text: &str) -> String {
        match self.style {
            TextStyle::Delete | TextStyle::Insert | TextStyle::Equal => text.to_string(),
            TextStyle::DeleteEmphasis => text.red().bold().underline().to_string(),
            TextStyle::InsertEmphasis => text.green().bold().underline().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TextStyle {
    Delete,
    Insert,
    Equal,
    DeleteEmphasis,
    InsertEmphasis,
}

fn push_wrapped_body(lines: &mut Vec<String>, label: &str, value: &str) {
    const WRAP_CHARS: usize = 118;
    let escaped = escape_visible(value);
    let content = format!("{label}{escaped}");

    for segment in wrap_chars(&content, WRAP_CHARS) {
        lines.push(format!("│ {segment}"));
    }
}

fn wrap_chars(value: &str, limit: usize) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut remaining = value.trim_end();

    while remaining.chars().count() > limit {
        let split_at = split_boundary(remaining, limit);
        let (line, rest) = remaining.split_at(split_at);
        lines.push(line.trim_end().to_string());
        remaining = rest.trim_start();
    }

    if !remaining.is_empty() {
        lines.push(remaining.to_string());
    }

    lines
}

fn split_boundary(value: &str, limit: usize) -> usize {
    let hard_limit = value
        .char_indices()
        .nth(limit)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    let candidate = &value[..hard_limit];

    candidate
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(idx, _)| idx)
        .filter(|idx| *idx > 0)
        .unwrap_or(hard_limit)
}

fn escape_visible(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());

    for ch in value.chars() {
        match ch {
            '\n' => rendered.push('⏎'),
            '\t' => rendered.push('⇥'),
            '\r' => rendered.push('␍'),
            _ => rendered.push(ch),
        }
    }

    rendered
}
