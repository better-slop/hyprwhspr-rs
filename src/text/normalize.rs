use super::cleanup::{
    capitalize_after_period, clean_control_artifacts, collapse_spaces, collapse_underscore_spacing,
    merge_separated_identical_symbols, normalize_line_breaks, trim_spaces_around_newlines,
};
use crate::logging::{record_text_pipeline, PipelineStepRecord, TextPipelineRecord};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use text_processing_rs::{custom_rules, normalize_sentence_with_options, NormalizeOptions};

static CUSTOM_RULES_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizeRule {
    spoken: String,
    written: String,
}

const APP_NORMALIZATION_RULES: &[(&str, &str)] = &[
    ("new line", "\n"),
    ("newline", "\n"),
    ("tab", "\t"),
    ("dash dash", "--"),
    ("dash", "-"),
    ("hyphen", "-"),
    ("underscore", "_"),
    ("under score", "_"),
    ("open paren", "("),
    ("open parenthesis", "("),
    ("open parentheses", "("),
    ("close paren", ")"),
    ("close parenthesis", ")"),
    ("close parentheses", ")"),
    ("open bracket", "["),
    ("close bracket", "]"),
    ("open brace", "{"),
    ("close brace", "}"),
    ("at symbol", "@"),
    ("at sign", "@"),
    ("hash", "#"),
    ("hash tag", "#"),
    ("hashtag", "#"),
    ("pound", "#"),
    ("dollar sign", "$"),
    ("percent", "%"),
    ("caret", "^"),
    ("ampersand", "&"),
    ("asterisk", "*"),
    ("plus", "+"),
    ("equals", "="),
    ("equal", "="),
    ("less than", "<"),
    ("greater than", ">"),
    ("slash", "/"),
    ("backslash", "\\"),
    ("pipe", "|"),
    ("tilde", "~"),
    ("grave", "`"),
    ("quote", "\""),
    ("double quote", "\""),
    ("apostrophe", "'"),
    ("single quote", "'"),
];

#[derive(Debug, Clone)]
pub struct NormalizeTextService {
    overrides: Vec<NormalizeRule>,
    options: NormalizeOptions,
}

impl NormalizeTextService {
    pub fn new(word_overrides: HashMap<String, String>) -> Self {
        Self {
            overrides: sorted_overrides(word_overrides),
            options: NormalizeOptions::new().with_disable_bare_second(true),
        }
    }

    pub fn normalize(&self, text: &str) -> String {
        let mut steps = if tracing::level_enabled!(tracing::Level::DEBUG) {
            Some(Vec::new())
        } else {
            None
        };
        let mut current = text.to_string();

        self.record_step(
            &mut steps,
            "normalize_line_breaks",
            &mut current,
            normalize_line_breaks,
            None,
        );

        let (after_itn, rule_count) = self.normalize_with_custom_rules(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "inverse_text_normalization",
                current.clone(),
                after_itn.clone(),
                if rule_count > 0 {
                    Some(rule_count)
                } else {
                    None
                },
            ));
        }
        current = after_itn;

        self.record_step(
            &mut steps,
            "control_artifact_cleanup",
            &mut current,
            clean_control_artifacts,
            None,
        );
        self.record_step(
            &mut steps,
            "collapse_spaces",
            &mut current,
            collapse_spaces,
            None,
        );

        let (newline_cleaned, newline_trim_count) = trim_spaces_around_newlines(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "trim_spaces_around_newlines",
                current.clone(),
                newline_cleaned.clone(),
                if newline_trim_count > 0 {
                    Some(newline_trim_count)
                } else {
                    None
                },
            ));
        }
        current = newline_cleaned;

        let (merged_symbols, merge_count) = merge_separated_identical_symbols(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "merge_identical_symbols",
                current.clone(),
                merged_symbols.clone(),
                if merge_count > 0 {
                    Some(merge_count)
                } else {
                    None
                },
            ));
        }
        current = merged_symbols;

        let (bridged_underscores, underscore_count) = collapse_underscore_spacing(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "collapse_underscore_spacing",
                current.clone(),
                bridged_underscores.clone(),
                if underscore_count > 0 {
                    Some(underscore_count)
                } else {
                    None
                },
            ));
        }
        current = bridged_underscores;

        let (capitalized, capitalized_count) = capitalize_after_period(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "capitalize_after_period",
                current.clone(),
                capitalized.clone(),
                if capitalized_count > 0 {
                    Some(capitalized_count)
                } else {
                    None
                },
            ));
        }
        current = capitalized;

        let (after_overrides, override_count) = self.apply_overrides(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "word_overrides",
                current.clone(),
                after_overrides.clone(),
                if override_count > 0 {
                    Some(override_count)
                } else {
                    None
                },
            ));
        }
        current = after_overrides;

        let trimmed = current.trim().to_string();
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "trim_whitespace",
                current.clone(),
                trimmed.clone(),
                None,
            ));
        }

        if let Some(logged_steps) = steps {
            record_text_pipeline(TextPipelineRecord::new(
                text.to_string(),
                trimmed.clone(),
                logged_steps,
            ));
        }

        trimmed
    }

    fn normalize_with_custom_rules(&self, text: &str) -> (String, usize) {
        let _guard = CUSTOM_RULES_LOCK
            .lock()
            .expect("text-processing-rs custom rules lock poisoned");

        custom_rules::clear_rules();
        for (spoken, written) in APP_NORMALIZATION_RULES {
            custom_rules::add_rule(spoken, written);
        }
        for rule in &self.overrides {
            custom_rules::add_rule(&rule.spoken, &rule.written);
        }

        let result = normalize_sentence_with_options(text, self.options);
        let rule_count = custom_rules::rule_count();
        custom_rules::clear_rules();
        (result, rule_count)
    }

    fn apply_overrides(&self, text: &str) -> (String, usize) {
        let mut result = text.to_string();
        let mut count = 0;

        for rule in &self.overrides {
            count += replace_case_insensitive_word(&mut result, &rule.spoken, &rule.written);
            if let Some(capitalized) = capitalized_ascii_word(&rule.written) {
                count += replace_case_sensitive_word(&mut result, &capitalized, &rule.written);
            }
        }

        (result, count)
    }

    fn record_step(
        &self,
        steps: &mut Option<Vec<PipelineStepRecord>>,
        name: &'static str,
        current: &mut String,
        f: fn(&str) -> String,
        count: Option<usize>,
    ) {
        let next = f(current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                name,
                current.clone(),
                next.clone(),
                count,
            ));
        }
        *current = next;
    }
}

fn sorted_overrides(overrides: HashMap<String, String>) -> Vec<NormalizeRule> {
    let mut rules = overrides
        .into_iter()
        .filter_map(|(spoken, written)| {
            let spoken = spoken.trim().to_string();
            if spoken.is_empty() {
                None
            } else {
                Some(NormalizeRule { spoken, written })
            }
        })
        .collect::<Vec<_>>();

    rules.sort_by(|a, b| {
        b.spoken
            .split_whitespace()
            .count()
            .cmp(&a.spoken.split_whitespace().count())
            .then_with(|| b.spoken.len().cmp(&a.spoken.len()))
            .then_with(|| a.spoken.to_lowercase().cmp(&b.spoken.to_lowercase()))
            .then_with(|| a.spoken.cmp(&b.spoken))
    });
    rules
}

fn replace_case_insensitive_word(buffer: &mut String, from: &str, to: &str) -> usize {
    let pattern = format!(r"\b{}\b", regex::escape(from));
    let Ok(regex) = Regex::new(&format!("(?i){pattern}")) else {
        return 0;
    };
    replace_with_regex(buffer, &regex, to)
}

fn replace_case_sensitive_word(buffer: &mut String, from: &str, to: &str) -> usize {
    let pattern = format!(r"\b{}\b", regex::escape(from));
    let Ok(regex) = Regex::new(&pattern) else {
        return 0;
    };
    replace_with_regex(buffer, &regex, to)
}

fn replace_with_regex(buffer: &mut String, regex: &Regex, replacement: &str) -> usize {
    let count = regex.find_iter(buffer).count();
    if count > 0 {
        *buffer = regex.replace_all(buffer, replacement).to_string();
    }
    count
}

fn capitalized_ascii_word(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    let rest = chars.as_str();
    if !rest.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-') {
        return None;
    }

    Some(format!("{}{}", first.to_ascii_uppercase(), rest))
}

#[cfg(test)]
#[path = "normalize_tests.rs"]
mod tests;
