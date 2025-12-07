use super::{backup_file, xdg_config_home};
use anyhow::Result;
use owo_colors::OwoColorize;
use std::fs;
use std::process::Command;

const ELEPHANT_MENU: &str = include_str!("../../config/elephant/hyprwhspr.lua");

pub fn install(force: bool) -> Result<()> {
    println!("{}", "Installing Elephant menu...".blue());

    let elephant_dir = xdg_config_home().join("elephant/menus");
    fs::create_dir_all(&elephant_dir)?;

    let dst = elephant_dir.join("hyprwhspr.lua");

    // Check if already installed and identical
    if dst.exists() && !force {
        let existing = fs::read_to_string(&dst)?;
        if existing == ELEPHANT_MENU {
            println!("  {} Menu file already up to date", "○".yellow());
            return Ok(());
        }
        backup_file(&dst)?;
    }

    fs::write(&dst, ELEPHANT_MENU)?;
    println!("  {} Installed: {}", "✓".green(), dst.display());

    // Check if elephant is available
    let elephant_check = Command::new("which").arg("elephant").output();
    if !elephant_check.map(|o| o.status.success()).unwrap_or(false) {
        println!(
            "  {} Elephant not found in PATH",
            "○".yellow()
        );
        println!("  Install from: https://github.com/abenz1267/elephant");
    }

    Ok(())
}
