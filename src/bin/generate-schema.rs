use anyhow::{Context, Result};
use hyprwhspr_rs::config::generated_schema_json;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<()> {
    let output_path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/schema.json"));

    let schema = generated_schema_json()?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(&output_path, format!("{schema}\n"))
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    Ok(())
}
