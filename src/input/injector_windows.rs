use anyhow::{Context, Result};
use arboard::Clipboard;
use enigo::{Keyboard, Settings};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{debug, info};

static MULTISPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+").expect("valid multispace regex"));
static WORD_BOUNDARY_TEMPLATE: LazyLock<String> = LazyLock::new(|| String::from(r"\b{}\b"));

const SPEECH_REPLACEMENTS: &[(&str, &str)] = &[
    ("new line", "\n"),
    ("newline", "\n"),
    ("tab", "\t"),
    ("period", "."),
    ("comma", ","),
    ("question mark", "?"),
    ("exclamation mark", "!"),
    ("exclamation point", "!"),
    ("colon", ":"),
    ("semicolon", ";"),
];

pub struct TextInjector {
    clipboard: Clipboard,
    word_overrides: HashMap<String, String>,
    default_shift_paste: bool,
    global_paste_shortcut: bool,
}

impl TextInjector {
    pub fn new(
        shift_paste_default: bool,
        global_paste_shortcut: bool,
        _extra_shift_classes: Vec<String>,
        _extra_shift_insert_classes: Vec<String>,
        word_overrides: HashMap<String, String>,
        _auto_copy_clipboard: bool,
    ) -> Result<Self> {
        let clipboard = Clipboard::new().context("Failed to initialize clipboard")?;

        Ok(Self {
            clipboard,
            word_overrides: sanitize_word_overrides(word_overrides),
            default_shift_paste: shift_paste_default,
            global_paste_shortcut,
        })
    }

    pub async fn inject_text(&mut self, text: &str) -> Result<()> {
        let processed = self.preprocess_text(text);
        if processed.is_empty() {
            debug!("No text to inject after preprocessing");
            return Ok(());
        }

        self.clipboard
            .set_text(processed)
            .context("Failed to copy text to clipboard")?;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        if self.global_paste_shortcut {
            self.inject_shift_insert()?;
        } else if self.default_shift_paste {
            self.inject_ctrl_shift_v()?;
        } else {
            self.inject_ctrl_v()?;
        }

        Ok(())
    }

    fn preprocess_text(&self, text: &str) -> String {
        let mut output = text.replace("\r\n", "\n").replace('\r', "\n");

        for (from, to) in &self.word_overrides {
            let pattern = WORD_BOUNDARY_TEMPLATE.replace("{}", &regex::escape(from));
            if let Ok(regex) = Regex::new(&format!("(?i){pattern}")) {
                output = regex.replace_all(&output, to.as_str()).into_owned();
            }
        }

        for (from, to) in SPEECH_REPLACEMENTS {
            let pattern = WORD_BOUNDARY_TEMPLATE.replace("{}", &regex::escape(from));
            if let Ok(regex) = Regex::new(&format!("(?i){pattern}")) {
                output = regex.replace_all(&output, *to).into_owned();
            }
        }

        let mut cleaned_lines = Vec::new();
        for line in output.lines() {
            cleaned_lines.push(MULTISPACE_REGEX.replace_all(line.trim(), " ").into_owned());
        }

        cleaned_lines.join("\n").trim().to_string()
    }

    fn inject_ctrl_v(&mut self) -> Result<()> {
        use enigo::{Direction, Key};

        let mut enigo =
            enigo::Enigo::new(&Settings::default()).context("Failed to initialize Enigo")?;

        enigo
            .key(Key::Control, Direction::Press)
            .context("Failed to press Ctrl")?;
        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .context("Failed to press V")?;
        enigo
            .key(Key::Control, Direction::Release)
            .context("Failed to release Ctrl")?;

        info!("Text injected via Windows clipboard paste");
        Ok(())
    }

    fn inject_ctrl_shift_v(&mut self) -> Result<()> {
        use enigo::{Direction, Key};

        let mut enigo =
            enigo::Enigo::new(&Settings::default()).context("Failed to initialize Enigo")?;

        enigo
            .key(Key::Control, Direction::Press)
            .context("Failed to press Ctrl")?;
        enigo
            .key(Key::Shift, Direction::Press)
            .context("Failed to press Shift")?;
        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .context("Failed to press V")?;
        enigo
            .key(Key::Shift, Direction::Release)
            .context("Failed to release Shift")?;
        enigo
            .key(Key::Control, Direction::Release)
            .context("Failed to release Ctrl")?;

        info!("Text injected via Windows terminal paste");
        Ok(())
    }

    fn inject_shift_insert(&mut self) -> Result<()> {
        use enigo::{Direction, Key};

        let mut enigo =
            enigo::Enigo::new(&Settings::default()).context("Failed to initialize Enigo")?;

        enigo
            .key(Key::Shift, Direction::Press)
            .context("Failed to press Shift")?;
        enigo
            .key(Key::Insert, Direction::Click)
            .context("Failed to press Insert")?;
        enigo
            .key(Key::Shift, Direction::Release)
            .context("Failed to release Shift")?;

        info!("Text injected via Windows Shift+Insert paste");
        Ok(())
    }
}

fn sanitize_word_overrides(word_overrides: HashMap<String, String>) -> HashMap<String, String> {
    word_overrides
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_string();
            if key.is_empty() {
                None
            } else {
                Some((key, value))
            }
        })
        .collect()
}
