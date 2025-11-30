use std::env;
use std::path::PathBuf;

/// Expands a leading `~/` in a path to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed.starts_with("~/") {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join(&trimmed[2..]);
        }
    }
    PathBuf::from(trimmed)
}
