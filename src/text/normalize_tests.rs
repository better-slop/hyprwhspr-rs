use super::*;
use std::collections::HashMap;
use std::sync::Arc;

fn service(overrides: &[(&str, &str)]) -> NormalizeTextService {
    NormalizeTextService::new(
        overrides
            .iter()
            .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
            .collect(),
    )
}

#[derive(Debug)]
struct FakeItnEngine {
    output: &'static str,
    rule_count: usize,
}

impl ItnEngine for FakeItnEngine {
    fn normalize(&self, _text: &str, _overrides: &[NormalizeRule]) -> ItnOutput {
        ItnOutput {
            text: self.output.to_string(),
            rule_count: self.rule_count,
        }
    }
}

#[test]
fn itn_normalizes_numbers_money_and_dates() {
    let normalizer = service(&[]);

    assert_eq!(normalizer.normalize("two hundred dollars"), "$200");
    assert_eq!(
        normalizer.normalize("january fifth twenty twenty five"),
        "January 5 2025"
    );
}

#[test]
fn itn_normalizes_commanded_punctuation() {
    let normalizer = service(&[]);
    let input = "this is awesome period i love this comma fuck yeah comma fuck period";

    assert_eq!(
        normalizer.normalize(input),
        "This is awesome. I love this, fuck yeah, fuck."
    );
}

#[test]
fn app_rules_preserve_code_dictation_symbols() {
    let normalizer = service(&[]);

    assert_eq!(normalizer.normalize("hello new line world"), "Hello\nWorld");
    assert_eq!(normalizer.normalize("tab item"), "\tItem");
    assert_eq!(
        normalizer.normalize("prepare dash dash go"),
        "Prepare -- go"
    );
    assert_eq!(
        normalizer.normalize("open brace foo comma close brace"),
        "{ foo }"
    );
    assert_eq!(normalizer.normalize("foo under score bar"), "Foo_bar");
    assert_eq!(
        normalizer.normalize("open quote hello close quote"),
        "\"hello\""
    );
    assert_eq!(
        normalizer.normalize("open single quote hello close single quote"),
        "'hello'"
    );
    assert_eq!(
        normalizer.normalize("open quote, yes, close quote."),
        "\"yes\""
    );
    assert_eq!(
        normalizer.normalize("open double quote, yes, close double quote."),
        "\"yes\""
    );
    assert_eq!(
        normalizer.normalize("single quote, yes, single quote."),
        "'yes'"
    );
    assert_eq!(
        normalizer.normalize("open single quote, yes, yes, yes, close single quote."),
        "'yes, yes, yes'"
    );
}

#[test]
fn word_overrides_feed_itn_custom_rules_and_keep_em_dash() {
    let normalizer = service(&[
        ("Hyperland", "hyprland"),
        ("em dash", "—"),
        ("gee pee tee", "GPT"),
    ]);

    assert_eq!(normalizer.normalize("Hyperland"), "hyprland");
    assert_eq!(normalizer.normalize("em dash"), "—");
    assert_eq!(normalizer.normalize("gee pee tee"), "GPT");
}

#[test]
fn fake_itn_engine_exercises_cleanup_and_override_order_without_globals() {
    let normalizer = NormalizeTextService::with_itn_engine(
        HashMap::from([("Hyperland".to_string(), "hyprland".to_string())]),
        Arc::new(FakeItnEngine {
            output: "\" , Hyperland, \".",
            rule_count: 9,
        }),
    );

    assert_eq!(normalizer.normalize("ignored"), "\"hyprland\"");
}

#[test]
fn normalization_pipeline_order_is_explicit() {
    assert_eq!(
        NormalizeTextService::pipeline_step_names(),
        vec![
            "normalize_line_breaks",
            "inverse_text_normalization",
            "control_commands",
            "control_artifact_cleanup",
            "collapse_spaces",
            "trim_spaces_around_newlines",
            "merge_identical_symbols",
            "collapse_underscore_spacing",
            "capitalize_after_period",
            "word_overrides",
            "trim_whitespace",
        ]
    );
}

#[test]
fn word_overrides_sort_longest_first_then_lexical() {
    let rules = sorted_overrides(HashMap::from([
        ("alpha".to_string(), "A".to_string()),
        ("alpha beta".to_string(), "AB".to_string()),
        ("alpha alpha".to_string(), "AA".to_string()),
    ]));

    let spoken = rules
        .iter()
        .map(|rule| rule.spoken.as_str())
        .collect::<Vec<_>>();
    assert_eq!(spoken, vec!["alpha alpha", "alpha beta", "alpha"]);
}

#[test]
fn trims_empty_after_normalization() {
    let normalizer = service(&[("erase me", "")]);
    assert_eq!(normalizer.normalize("erase me"), "");
}

#[test]
fn control_cleanup_collapses_symbol_punctuation() {
    assert_eq!(clean_control_artifacts("(, value ,)"), "(value)");
    assert_eq!(clean_control_artifacts("[, option ,]"), "[ option ]");
    assert_eq!(clean_control_artifacts("{, field ,}"), "{ field }");
}

#[test]
fn merge_identical_symbols_collapses_spaced_pairs() {
    let input = "77 - - go and _ _ done";
    let (merged, count) = merge_separated_identical_symbols(input);
    assert_eq!(merged, "77 -- go and __ done");
    assert_eq!(count, 2);
}
