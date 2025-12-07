use super::{backup_file, xdg_config_home};
use anyhow::Result;
use owo_colors::OwoColorize;
use std::fs;
use std::process::Command;

const SYSTEMD_SERVICE: &str = include_str!("../../config/systemd/hyprwhspr-rs.service");

pub fn install(force: bool) -> Result<()> {
    println!("{}", "Installing systemd service...".blue());

    let systemd_dir = xdg_config_home().join("systemd/user");
    fs::create_dir_all(&systemd_dir)?;

    let dst = systemd_dir.join("hyprwhspr-rs.service");

    // Check if already installed and identical
    if dst.exists() && !force {
        let existing = fs::read_to_string(&dst)?;
        if existing == SYSTEMD_SERVICE {
            println!("  {} Service file already up to date", "○".yellow());
            daemon_reload_enable_start()?;
            return Ok(());
        }
        backup_file(&dst)?;
    }

    fs::write(&dst, SYSTEMD_SERVICE)?;
    println!("  {} Installed: {}", "✓".green(), dst.display());

    daemon_reload_enable_start()?;

    Ok(())
}

fn daemon_reload_enable_start() -> Result<()> {
    // Reload systemd
    let reload = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    if let Err(e) = reload {
        println!(
            "  {} Failed to reload systemd: {}",
            "✗".red(),
            e
        );
        println!("  Run manually: systemctl --user daemon-reload");
        return Ok(());
    }

    // Enable service
    let enable = Command::new("systemctl")
        .args(["--user", "enable", "hyprwhspr-rs.service"])
        .output();

    match enable {
        Ok(out) if out.status.success() => {
            println!("  {} Service enabled", "✓".green());
        }
        Ok(out) => {
            println!(
                "  {} Failed to enable service: {}",
                "✗".red(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(e) => {
            println!("  {} Failed to enable service: {}", "✗".red(), e);
        }
    }

    // Start/restart service
    let is_active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "hyprwhspr-rs.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let action = if is_active { "restart" } else { "start" };

    let start = Command::new("systemctl")
        .args(["--user", action, "hyprwhspr-rs.service"])
        .output();

    match start {
        Ok(out) if out.status.success() => {
            println!(
                "  {} Service {}ed",
                "✓".green(),
                action
            );
        }
        Ok(out) => {
            println!(
                "  {} Failed to {} service: {}",
                "✗".red(),
                action,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            println!("  Check: systemctl --user status hyprwhspr-rs");
        }
        Err(e) => {
            println!("  {} Failed to {} service: {}", "✗".red(), action, e);
        }
    }

    Ok(())
}
