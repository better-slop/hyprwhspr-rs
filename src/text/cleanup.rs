use regex::Regex;
use std::sync::LazyLock;

static SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" +").expect("valid space collapse regex"));
static CONTROL_PUNCT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([\n\t])\s*[.!?,;:]+").expect("valid control artifact cleanup regex")
});
static CONTROL_TRAILING_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+([\n\t])").expect("valid trailing space cleanup regex"));
static SYMBOL_PUNCT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([()\[\]\{\}])\s*[.,;]+").expect("valid symbol artifact cleanup regex")
});
static OPEN_PAREN_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\( +").expect("valid open paren space cleanup regex"));
static CLOSE_PAREN_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" +\)").expect("valid close paren space cleanup regex"));
static OPEN_PAREN_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*,\s*").expect("valid open paren comma cleanup regex"));
static CLOSE_PAREN_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*,\s*\)").expect("valid close paren comma cleanup regex"));
static OPEN_BRACKET_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\s*,\s*").expect("valid open bracket comma cleanup regex"));
static CLOSE_BRACKET_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*,\s*\]").expect("valid close bracket comma cleanup regex"));
static OPEN_BRACE_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\s*,\s*").expect("valid open brace comma cleanup regex"));
static CLOSE_BRACE_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*,\s*\}").expect("valid close brace comma cleanup regex"));
static SPACE_BEFORE_PUNCT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ \t]+([,.;:!?])").expect("valid punctuation spacing cleanup regex")
});
static DUPLICATE_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",(?:\s*,)+").expect("valid duplicate comma cleanup regex"));
static SPACE_BEFORE_NEWLINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+\n").expect("valid space before newline regex"));
static SPACE_AFTER_NEWLINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n[ \t]+").expect("valid space after newline regex"));
const MERGE_SYMBOLS: &[char] = &['-', '_', '+', '*', '/', '=', '~', '^'];
static MERGE_SYMBOL_PATTERNS: LazyLock<Vec<(char, Regex)>> = LazyLock::new(|| {
    MERGE_SYMBOLS
        .iter()
        .map(|sym| {
            let escaped = regex::escape(&sym.to_string());
            let pattern = format!(r"{escaped}\s+{escaped}");
            (
                *sym,
                Regex::new(&pattern)
                    .expect("valid identical symbol merge regex for specific symbol"),
            )
        })
        .collect()
});
static UNDERSCORE_BRIDGE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([^\s_])\s+(_+)\s+([^\s_])").expect("valid underscore bridge regex")
});

pub(super) fn normalize_line_breaks(input: &str) -> String {
    input
        .replace("\r\n", " ")
        .replace('\r', " ")
        .replace('\n', " ")
}

pub(super) fn collapse_spaces(input: &str) -> String {
    SPACE_REGEX.replace_all(input, " ").to_string()
}

pub(super) fn clean_control_artifacts(input: &str) -> String {
    let without_control_punct = CONTROL_PUNCT_REGEX.replace_all(input, "$1");
    let without_trailing_space =
        CONTROL_TRAILING_SPACE_REGEX.replace_all(&without_control_punct, "$1");
    let without_symbol_punct = SYMBOL_PUNCT_REGEX.replace_all(&without_trailing_space, "$1");
    let collapsed_open = OPEN_PAREN_SPACE_REGEX.replace_all(&without_symbol_punct, "(");
    let collapsed_close = CLOSE_PAREN_SPACE_REGEX.replace_all(&collapsed_open, ")");
    let no_open_comma = OPEN_PAREN_COMMA_REGEX.replace_all(&collapsed_close, "(");
    let no_close_comma = CLOSE_PAREN_COMMA_REGEX.replace_all(&no_open_comma, ")");
    let no_open_bracket_comma = OPEN_BRACKET_COMMA_REGEX.replace_all(&no_close_comma, "[ ");
    let no_close_bracket_comma =
        CLOSE_BRACKET_COMMA_REGEX.replace_all(&no_open_bracket_comma, " ]");
    let no_open_brace_comma = OPEN_BRACE_COMMA_REGEX.replace_all(&no_close_bracket_comma, "{ ");
    let no_close_brace_comma = CLOSE_BRACE_COMMA_REGEX.replace_all(&no_open_brace_comma, " }");
    let no_space_before_punct = SPACE_BEFORE_PUNCT_REGEX.replace_all(&no_close_brace_comma, "$1");
    DUPLICATE_COMMA_REGEX
        .replace_all(&no_space_before_punct, ",")
        .to_string()
}

pub(super) fn capitalize_after_period(input: &str) -> (String, usize) {
    let mut result = String::with_capacity(input.len());
    let mut capitalize_next = true;
    let mut awaiting_space_after_punct = false;
    let mut count = 0;

    for ch in input.chars() {
        if awaiting_space_after_punct {
            if ch == ' ' {
                capitalize_next = true;
            } else if !ch.is_whitespace() {
                awaiting_space_after_punct = false;
            }
        }

        let mut output_char = ch;

        if capitalize_next {
            if ch.is_ascii_lowercase() {
                output_char = ch.to_ascii_uppercase();
                count += 1;
                capitalize_next = false;
                awaiting_space_after_punct = false;
            } else if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
                capitalize_next = false;
                awaiting_space_after_punct = false;
            } else if !ch.is_whitespace() {
                capitalize_next = false;
                awaiting_space_after_punct = false;
            }
        }

        result.push(output_char);

        match ch {
            '.' | '!' | '?' => {
                capitalize_next = false;
                awaiting_space_after_punct = true;
            }
            '\n' => {
                capitalize_next = true;
                awaiting_space_after_punct = false;
            }
            _ => {}
        }
    }

    (result, count)
}

pub(super) fn merge_separated_identical_symbols(input: &str) -> (String, usize) {
    let mut total_count = 0;
    let mut current = input.to_string();

    for (sym, regex) in MERGE_SYMBOL_PATTERNS.iter() {
        let replacement = format!("{sym}{sym}");

        loop {
            let matches = regex.find_iter(&current).count();
            if matches == 0 {
                break;
            }

            total_count += matches;
            current = regex
                .replace_all(&current, replacement.as_str())
                .into_owned();
        }
    }

    (current, total_count)
}

pub(super) fn collapse_underscore_spacing(input: &str) -> (String, usize) {
    let mut total_count = 0;
    let mut current = input.to_string();

    loop {
        let matches = UNDERSCORE_BRIDGE_REGEX.captures_iter(&current).count();
        if matches == 0 {
            break;
        }

        total_count += matches;
        current = UNDERSCORE_BRIDGE_REGEX
            .replace_all(&current, "$1$2$3")
            .into_owned();
    }

    (current, total_count)
}

pub(super) fn trim_spaces_around_newlines(input: &str) -> (String, usize) {
    let mut count = 0;

    let trailing_matches = SPACE_BEFORE_NEWLINE_REGEX.find_iter(input).count();
    let without_trailing = SPACE_BEFORE_NEWLINE_REGEX
        .replace_all(input, "\n")
        .into_owned();
    count += trailing_matches;

    let leading_matches = SPACE_AFTER_NEWLINE_REGEX
        .find_iter(&without_trailing)
        .count();
    let final_result = SPACE_AFTER_NEWLINE_REGEX
        .replace_all(&without_trailing, "\n")
        .into_owned();
    count += leading_matches;

    (final_result, count)
}
