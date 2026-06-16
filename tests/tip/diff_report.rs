use owo_colors::OwoColorize;
use similar::{Algorithm, ChangeTag, DiffableStr, InlineChange, TextDiff};

pub(crate) fn assert_text_eq(label: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }

    panic!(
        "{label} mismatch\n\n{}\n\n{}",
        render_wrapped_text_pipeline_diff(expected, actual),
        render_similar_report(expected, actual)
    );
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
    hard_wrap_ansi_report(&out)
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
        .map(|line| {
            if line.starts_with("---") || line.starts_with("+++") {
                line.bold().to_string()
            } else if line.starts_with("@@") {
                line.cyan().to_string()
            } else if line.starts_with('+') {
                line.green().to_string()
            } else if line.starts_with('-') {
                line.red().to_string()
            } else if line.starts_with("\\ ") {
                line.dimmed().to_string()
            } else {
                line.dimmed().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let prefix = format!(
        "{} {} │{}│",
        line_no(change.old_index()),
        line_no(change.new_index()),
        sign
    );
    let body = render_inline_change(change);
    let newline_marker = if change.missing_newline() {
        format!(" {}", "\\ No newline at end of file".dimmed())
    } else {
        String::new()
    };

    match change.tag() {
        ChangeTag::Delete => format!("{} {}{}", prefix.red(), body, newline_marker),
        ChangeTag::Insert => format!("{} {}{}", prefix.green(), body, newline_marker),
        ChangeTag::Equal => format!("{} {}{}", prefix.dimmed(), body.dimmed(), newline_marker),
    }
}

fn line_no(index: Option<usize>) -> String {
    index
        .map(|idx| format!("{:>4}", idx + 1))
        .unwrap_or_else(|| "    ".to_string())
}

fn render_inline_change<T>(change: &InlineChange<'_, T>) -> String
where
    T: DiffableStr + ?Sized,
{
    let mut out = String::new();

    for (emphasized, value) in change.iter_strings_lossy() {
        let escaped = escape_visible(&value);
        let rendered = match (change.tag(), emphasized) {
            (ChangeTag::Delete, true) => escaped.red().bold().underline().to_string(),
            (ChangeTag::Insert, true) => escaped.green().bold().underline().to_string(),
            (ChangeTag::Delete, false) => escaped.red().to_string(),
            (ChangeTag::Insert, false) => escaped.green().to_string(),
            (ChangeTag::Equal, _) => escaped.dimmed().to_string(),
        };
        out.push_str(&rendered);
    }

    out.trim_end_matches('⏎').to_string()
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

fn hard_wrap_ansi_report(report: &str) -> String {
    const WIDTH: usize = 118;
    const CONTINUATION: &str = "      ";

    report
        .lines()
        .flat_map(|line| hard_wrap_ansi_line(line, WIDTH, CONTINUATION))
        .collect::<Vec<_>>()
        .join("\n")
}

fn hard_wrap_ansi_line(line: &str, width: usize, continuation: &str) -> Vec<String> {
    if visible_width(line) <= width {
        return vec![line.to_string()];
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let mut visible = 0usize;
    let mut byte_idx = 0usize;

    while byte_idx < line.len() {
        let rest = &line[byte_idx..];
        if rest.starts_with('\x1b') {
            let sequence_len = ansi_sequence_len(rest);
            current.push_str(&rest[..sequence_len]);
            byte_idx += sequence_len;
            continue;
        }

        let ch = rest.chars().next().expect("non-empty string has next char");
        current.push(ch);
        byte_idx += ch.len_utf8();
        visible += 1;

        if visible >= width && byte_idx < line.len() {
            out.push(current);
            current = continuation.to_string();
            visible = continuation.chars().count();
        }
    }

    if !current.is_empty() {
        out.push(current);
    }

    out
}

fn visible_width(line: &str) -> usize {
    let mut width = 0usize;
    let mut byte_idx = 0usize;

    while byte_idx < line.len() {
        let rest = &line[byte_idx..];
        if rest.starts_with('\x1b') {
            byte_idx += ansi_sequence_len(rest);
            continue;
        }

        let ch = rest.chars().next().expect("non-empty string has next char");
        byte_idx += ch.len_utf8();
        width += 1;
    }

    width
}

fn ansi_sequence_len(value: &str) -> usize {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&0x1b) {
        return 0;
    }

    let start = if bytes.get(1) == Some(&b'[') { 2 } else { 1 };

    for (idx, byte) in bytes.iter().enumerate().skip(start) {
        if (0x40..=0x7e).contains(byte) {
            return idx + 1;
        }
    }

    value.len()
}
