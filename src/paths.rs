use std::env;
use std::path::PathBuf;

/// Expands a leading `~/` in a path to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed.starts_with("~/") {
        if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(&trimmed[2..]);
        }
    }
    PathBuf::from(trimmed)
}
