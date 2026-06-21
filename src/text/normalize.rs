use super::cleanup::{
    capitalize_after_period, clean_control_artifacts, collapse_spaces, collapse_underscore_spacing,
    merge_separated_identical_symbols, normalize_line_breaks, trim_spaces_around_newlines,
};
use crate::logging::{PipelineStepRecord, TextPipelineRecord, record_text_pipeline};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use text_processing_rs::{NormalizeOptions, custom_rules, normalize_sentence_with_options};

static CUSTOM_RULES_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizeRule {
    spoken: String,
    written: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ItnOutput {
    text: String,
    rule_count: usize,
}

trait ItnEngine: std::fmt::Debug + Send + Sync {
    fn normalize(&self, text: &str, overrides: &[NormalizeRule]) -> ItnOutput;
}

#[derive(Debug, Clone)]
struct TextProcessingItnEngine {
    options: NormalizeOptions,
}

const APP_NORMALIZATION_RULES: &[(&str, &str)] = &[
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
    ("open quote", "\""),
    ("close quote", "\""),
    ("open double quote", "\""),
    ("close double quote", "\""),
    ("quote", "\""),
    ("double quote", "\""),
    ("open single quote", "'"),
    ("close single quote", "'"),
    ("apostrophe", "'"),
    ("single quote", "'"),
];

const CONTROL_COMMAND_RULES: &[(&str, &str)] =
    &[("new line", "\n"), ("newline", "\n"), ("tab", "\t")];

const NORMALIZE_PIPELINE: &[NormalizeStep] = &[
    NormalizeStep::LineBreaks,
    NormalizeStep::InverseTextNormalization,
    NormalizeStep::ControlCommands,
    NormalizeStep::ControlArtifactCleanup,
    NormalizeStep::CollapseSpaces,
    NormalizeStep::TrimSpacesAroundNewlines,
    NormalizeStep::MergeIdenticalSymbols,
    NormalizeStep::CollapseUnderscoreSpacing,
    NormalizeStep::CapitalizeAfterPeriod,
    NormalizeStep::WordOverrides,
    NormalizeStep::TrimWhitespace,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NormalizeStep {
    LineBreaks,
    InverseTextNormalization,
    ControlCommands,
    ControlArtifactCleanup,
    CollapseSpaces,
    TrimSpacesAroundNewlines,
    MergeIdenticalSymbols,
    CollapseUnderscoreSpacing,
    CapitalizeAfterPeriod,
    WordOverrides,
    TrimWhitespace,
}

struct StepOutput {
    text: String,
    change_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct NormalizeTextService {
    overrides: Vec<NormalizeRule>,
    itn: Arc<dyn ItnEngine>,
}

impl NormalizeTextService {
    pub fn new(word_overrides: HashMap<String, String>) -> Self {
        Self {
            overrides: sorted_overrides(word_overrides),
            itn: Arc::new(TextProcessingItnEngine::default()),
        }
    }

    #[cfg(test)]
    fn with_itn_engine(word_overrides: HashMap<String, String>, itn: Arc<dyn ItnEngine>) -> Self {
        Self {
            overrides: sorted_overrides(word_overrides),
            itn,
        }
    }

    pub fn normalize(&self, text: &str) -> String {
        let mut logged_steps = if tracing::level_enabled!(tracing::Level::DEBUG) {
            Some(Vec::new())
        } else {
            None
        };
        let mut current = text.to_string();

        for step in NORMALIZE_PIPELINE {
            let before = current;
            let output = step.apply(self, &before);
            if let Some(ref mut steps) = logged_steps {
                steps.push(PipelineStepRecord::new(
                    step.name(),
                    before.clone(),
                    output.text.clone(),
                    output.change_count,
                ));
            }
            current = output.text;
        }

        if let Some(logged_steps) = logged_steps {
            record_text_pipeline(TextPipelineRecord::new(
                text.to_string(),
                current.clone(),
                logged_steps,
            ));
        }

        current
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

    #[cfg(test)]
    fn pipeline_step_names() -> Vec<&'static str> {
        NORMALIZE_PIPELINE.iter().map(|step| step.name()).collect()
    }
}

impl NormalizeStep {
    fn name(self) -> &'static str {
        match self {
            Self::LineBreaks => "normalize_line_breaks",
            Self::InverseTextNormalization => "inverse_text_normalization",
            Self::ControlCommands => "control_commands",
            Self::ControlArtifactCleanup => "control_artifact_cleanup",
            Self::CollapseSpaces => "collapse_spaces",
            Self::TrimSpacesAroundNewlines => "trim_spaces_around_newlines",
            Self::MergeIdenticalSymbols => "merge_identical_symbols",
            Self::CollapseUnderscoreSpacing => "collapse_underscore_spacing",
            Self::CapitalizeAfterPeriod => "capitalize_after_period",
            Self::WordOverrides => "word_overrides",
            Self::TrimWhitespace => "trim_whitespace",
        }
    }

    fn apply(self, service: &NormalizeTextService, text: &str) -> StepOutput {
        match self {
            Self::LineBreaks => StepOutput::without_count(normalize_line_breaks(text)),
            Self::InverseTextNormalization => {
                let output = service.itn.normalize(text, &service.overrides);
                StepOutput::with_count(output.text, output.rule_count)
            }
            Self::ControlCommands => {
                let (text, count) = apply_control_commands(text);
                StepOutput::with_count(text, count)
            }
            Self::ControlArtifactCleanup => {
                StepOutput::without_count(clean_control_artifacts(text))
            }
            Self::CollapseSpaces => StepOutput::without_count(collapse_spaces(text)),
            Self::TrimSpacesAroundNewlines => {
                let (text, count) = trim_spaces_around_newlines(text);
                StepOutput::with_count(text, count)
            }
            Self::MergeIdenticalSymbols => {
                let (text, count) = merge_separated_identical_symbols(text);
                StepOutput::with_count(text, count)
            }
            Self::CollapseUnderscoreSpacing => {
                let (text, count) = collapse_underscore_spacing(text);
                StepOutput::with_count(text, count)
            }
            Self::CapitalizeAfterPeriod => {
                let (text, count) = capitalize_after_period(text);
                StepOutput::with_count(text, count)
            }
            Self::WordOverrides => {
                let (text, count) = service.apply_overrides(text);
                StepOutput::with_count(text, count)
            }
            Self::TrimWhitespace => StepOutput::without_count(text.trim_matches(' ').to_string()),
        }
    }
}

impl StepOutput {
    fn without_count(text: String) -> Self {
        Self {
            text,
            change_count: None,
        }
    }

    fn with_count(text: String, count: usize) -> Self {
        Self {
            text,
            change_count: (count > 0).then_some(count),
        }
    }
}

impl Default for TextProcessingItnEngine {
    fn default() -> Self {
        Self {
            options: NormalizeOptions::new().with_disable_bare_second(true),
        }
    }
}

impl ItnEngine for TextProcessingItnEngine {
    fn normalize(&self, text: &str, overrides: &[NormalizeRule]) -> ItnOutput {
        let _guard = CUSTOM_RULES_LOCK
            .lock()
            .expect("text-processing-rs custom rules lock poisoned");

        custom_rules::clear_rules();
        for (spoken, written) in APP_NORMALIZATION_RULES {
            custom_rules::add_rule(spoken, written);
        }
        for rule in overrides {
            custom_rules::add_rule(&rule.spoken, &rule.written);
        }

        let result = normalize_sentence_with_options(text, self.options);
        let rule_count = custom_rules::rule_count();
        custom_rules::clear_rules();

        ItnOutput {
            text: result,
            rule_count,
        }
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

fn apply_control_commands(text: &str) -> (String, usize) {
    let mut result = text.to_string();
    let mut count = 0;

    let mut rules = CONTROL_COMMAND_RULES.to_vec();
    rules.sort_by(|a, b| {
        b.0.split_whitespace()
            .count()
            .cmp(&a.0.split_whitespace().count())
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| a.0.cmp(b.0))
    });

    for (spoken, written) in rules {
        count += replace_case_insensitive_word(&mut result, spoken, written);
    }

    (result, count)
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
