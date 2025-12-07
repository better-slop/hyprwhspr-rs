use super::{backup_file, xdg_config_home};
use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use regex::Regex;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const WAYBAR_MODULE: &str = include_str!("../../config/waybar/hyprwhspr-module.jsonc");
const WAYBAR_CSS: &str = include_str!("../../config/waybar/hyprwhspr-style.css");

pub fn install(force: bool) -> Result<()> {
    println!("{}", "Installing Waybar module...".blue());

    install_module(force)?;
    install_css(force)?;
    reload_waybar()?;

    Ok(())
}

fn waybar_config_dir() -> PathBuf {
    xdg_config_home().join("waybar")
}

fn find_waybar_config() -> Option<PathBuf> {
    let dir = waybar_config_dir();
    for name in ["config.jsonc", "config.json", "config"] {
        let path = dir.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn install_module(_force: bool) -> Result<()> {
    let config_path = find_waybar_config().unwrap_or_else(|| {
        let path = waybar_config_dir().join("config.jsonc");
        println!(
            "  {} No waybar config found, creating {}",
            "○".yellow(),
            path.display()
        );
        path
    });

    // Read existing config or create empty
    let content = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        fs::create_dir_all(config_path.parent().unwrap())?;
        r#"{"modules-right": []}"#.to_string()
    };

    // Check if module definition already exists
    if content.contains(r#""custom/hyprwhspr""#) && content.contains("exec") {
        println!(
            "  {} Waybar module definition already exists",
            "○".yellow()
        );
        return Ok(());
    }

    // Backup existing config
    if config_path.exists() {
        backup_file(&config_path)?;
    }

    // Clean JSONC to JSON for parsing
    let json_clean = clean_jsonc(&content);

    // Parse and modify
    let mut config: serde_json::Value =
        serde_json::from_str(&json_clean).context("Failed to parse waybar config as JSON")?;

    // Parse module definition and add it
    let module_def: serde_json::Value = serde_json::from_str(&clean_jsonc(WAYBAR_MODULE))?;
    if let Some(def) = module_def.get("custom/hyprwhspr") {
        config["custom/hyprwhspr"] = def.clone();
    }

    // Add to modules-right if not present
    if let Some(modules) = config.get_mut("modules-right") {
        if let Some(arr) = modules.as_array_mut() {
            if !arr
                .iter()
                .any(|v| v.as_str() == Some("custom/hyprwhspr"))
            {
                arr.insert(0, serde_json::json!("custom/hyprwhspr"));
            }
        }
    } else if let Some(modules) = config.get_mut("modules-left") {
        if let Some(arr) = modules.as_array_mut() {
            if !arr
                .iter()
                .any(|v| v.as_str() == Some("custom/hyprwhspr"))
            {
                arr.insert(0, serde_json::json!("custom/hyprwhspr"));
            }
        }
    } else {
        config["modules-right"] = serde_json::json!(["custom/hyprwhspr"]);
    }

    // Write back
    let output = serde_json::to_string_pretty(&config)?;
    fs::write(&config_path, output)?;

    println!(
        "  {} Added hyprwhspr module to {}",
        "✓".green(),
        config_path.display()
    );
    Ok(())
}

fn install_css(_force: bool) -> Result<()> {
    let style_path = waybar_config_dir().join("style.css");

    let content = if style_path.exists() {
        fs::read_to_string(&style_path)?
    } else {
        String::new()
    };

    // Check if styles already exist
    if content.contains("#custom-hyprwhspr") {
        println!("  {} Waybar CSS already contains hyprwhspr styles", "○".yellow());
        return Ok(());
    }

    // Backup and append
    if style_path.exists() && !content.is_empty() {
        backup_file(&style_path)?;
    }

    let mut new_content = content;
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    new_content.push('\n');
    new_content.push_str(WAYBAR_CSS);

    fs::create_dir_all(style_path.parent().unwrap())?;
    fs::write(&style_path, new_content)?;

    println!("  {} Appended CSS to {}", "✓".green(), style_path.display());
    Ok(())
}

fn reload_waybar() -> Result<()> {
    // Check if waybar is running
    let output = Command::new("pgrep").arg("-x").arg("waybar").output();

    match output {
        Ok(out) if out.status.success() => {
            Command::new("pkill")
                .arg("-SIGUSR2")
                .arg("waybar")
                .output()?;
            println!("  {} Waybar reloaded", "✓".green());
        }
        _ => {
            println!("  {} Waybar not running - start it manually", "○".yellow());
        }
    }

    Ok(())
}

/// Strip JSONC features (comments, trailing commas) to make valid JSON
fn clean_jsonc(content: &str) -> String {
    // Remove // comments (but not :// in URLs)
    let re_line_comment = Regex::new(r"(?m)(?<!:)//.*$").unwrap();
    let result = re_line_comment.replace_all(content, "");

    // Remove /* */ comments
    let re_block_comment = Regex::new(r"(?s)/\*.*?\*/").unwrap();
    let result = re_block_comment.replace_all(&result, "");

    // Remove trailing commas before ] or }
    let re_trailing = Regex::new(r",(\s*[}\]])").unwrap();
    re_trailing.replace_all(&result, "$1").to_string()
}
