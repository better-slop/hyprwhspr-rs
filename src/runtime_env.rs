use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const ENV_FILE_OVERRIDE: &str = "HYPRWHSPR_ENV_FILE";

pub fn load_env_files() -> Result<()> {
    for path in candidate_env_files()? {
        if path.is_file() {
            load_env_file(&path)?;
        }
    }

    Ok(())
}

fn candidate_env_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if let Some(path) = env::var_os(ENV_FILE_OVERRIDE).map(PathBuf::from) {
        push_unique(&mut files, path);
    }

    let config_dir = directories::ProjectDirs::from("", "", "hyprwhspr-rs")
        .context("Failed to determine hyprwhspr-rs config directory")?
        .config_dir()
        .to_path_buf();

    push_unique(&mut files, config_dir.join(".env"));
    push_unique(&mut files, config_dir.join("env"));

    if let Ok(cwd) = env::current_dir() {
        push_unique(&mut files, cwd.join(".env"));
    }

    Ok(files)
}

fn push_unique(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.contains(&candidate) {
        paths.push(candidate);
    }
}

fn load_env_file(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read env file at {}", path.display()))?;

    for (index, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=').with_context(|| {
            format!("Invalid env assignment at {}:{}", path.display(), index + 1)
        })?;

        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        if env::var_os(key).is_some() {
            continue;
        }

        let value = parse_env_value(value.trim());
        env::set_var(key, value);
    }

    Ok(())
}

fn parse_env_value(value: &str) -> String {
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }

    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::{load_env_file, parse_env_value};
    use std::env;
    use std::fs;
    use std::sync::{LazyLock, Mutex};

    static ENV_TEST_GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn quoted_env_values_are_unwrapped() {
        assert_eq!(parse_env_value("\"value\""), "value");
        assert_eq!(parse_env_value("'value'"), "value");
        assert_eq!(parse_env_value("plain"), "plain");
    }

    #[test]
    fn env_file_does_not_override_existing_process_vars() {
        let _guard = ENV_TEST_GUARD.lock().expect("env mutex poisoned");

        let root = std::env::temp_dir().join(format!("hyprwhspr-env-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("failed to create temp dir");

        let env_file = root.join(".env");
        fs::write(
            &env_file,
            "GROQ_API_KEY=file-value\nGEMINI_API_KEY=\"quoted-value\"\n",
        )
        .expect("failed to write env file");

        env::set_var("GROQ_API_KEY", "process-value");
        env::remove_var("GEMINI_API_KEY");

        load_env_file(&env_file).expect("failed to load env file");

        assert_eq!(env::var("GROQ_API_KEY").as_deref(), Ok("process-value"));
        assert_eq!(env::var("GEMINI_API_KEY").as_deref(), Ok("quoted-value"));

        env::remove_var("GROQ_API_KEY");
        env::remove_var("GEMINI_API_KEY");
        let _ = fs::remove_dir_all(root);
    }
}
