pub mod elephant;
pub mod systemd;
pub mod waybar;

use crate::cli::InstallArgs;
use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect};
use owo_colors::OwoColorize;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::{env, fs};
use time::OffsetDateTime;

/// Components available for installation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Waybar,
    Systemd,
    Elephant,
}

impl Component {
    pub fn label(&self) -> &'static str {
        match self {
            Component::Waybar => "Waybar module + CSS",
            Component::Systemd => "Systemd user service",
            Component::Elephant => "Elephant menu (Walker)",
        }
    }

    pub fn all() -> &'static [Component] {
        &[Component::Waybar, Component::Systemd, Component::Elephant]
    }
}

/// Result of a file copy operation
#[derive(Debug)]
pub enum CopyResult {
    Copied,
    Skipped,
    Overwritten,
}

/// Run the install command
pub fn run_install(args: &InstallArgs) -> Result<()> {
    println!();
    println!("{}", "━".repeat(70));
    println!("  hyprwhspr-rs Integration Installer");
    println!("{}", "━".repeat(70));
    println!();

    let components = if args.has_specific_flags() {
        // Direct mode: use flags
        let mut selected = Vec::new();
        if args.all || args.waybar {
            selected.push(Component::Waybar);
        }
        if args.all || args.service {
            selected.push(Component::Systemd);
        }
        if args.all || args.elephant {
            selected.push(Component::Elephant);
        }
        selected
    } else {
        // Interactive mode
        if !io::stdin().is_terminal() {
            anyhow::bail!("No TTY available for interactive mode. Use --waybar, --service, --elephant, or --all flags.");
        }
        interactive_select()?
    };

    if components.is_empty() {
        println!("{} No components selected", "○".yellow());
        return Ok(());
    }

    // Create base directories
    create_directories()?;

    // Install selected components
    for component in &components {
        match component {
            Component::Waybar => waybar::install(args.force)?,
            Component::Systemd => systemd::install(args.force)?,
            Component::Elephant => elephant::install(args.force)?,
        }
    }

    print_summary(&components);
    Ok(())
}

fn interactive_select() -> Result<Vec<Component>> {
    let items: Vec<&str> = Component::all().iter().map(|c| c.label()).collect();

    let selections = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select components to install (Space to toggle, Enter to confirm)")
        .items(&items)
        .defaults(&[true, true, false]) // waybar + systemd on by default
        .interact()?;

    Ok(selections
        .iter()
        .map(|&i| Component::all()[i])
        .collect())
}

fn create_directories() -> Result<()> {
    let dirs = [
        xdg_cache_home().join("hyprwhspr-rs"),
        xdg_data_home().join("hyprwhspr-rs"),
        xdg_config_home().join("hyprwhspr-rs"),
    ];

    for dir in &dirs {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }

    Ok(())
}

fn print_summary(components: &[Component]) {
    println!();
    println!("{}", "━".repeat(70));
    println!("{} Installation complete!", "✓".green());
    println!("{}", "━".repeat(70));
    println!();

    println!("Installed:");
    for c in components {
        println!("  {} {}", "✓".green(), c.label());
    }
    println!();

    println!("Files:");
    println!("  Config:  {}/hyprwhspr-rs/", xdg_config_home().display());
    println!(
        "  Status:  {}/hyprwhspr-rs/status.json",
        xdg_cache_home().display()
    );
    println!(
        "  History: {}/hyprwhspr-rs/transcriptions.json",
        xdg_data_home().display()
    );
    println!();

    if components.contains(&Component::Systemd) {
        println!("Commands:");
        println!("  Check status:   systemctl --user status hyprwhspr-rs");
        println!("  View logs:      journalctl --user -u hyprwhspr-rs -f");
        println!("  Restart:        systemctl --user restart hyprwhspr-rs");
        println!();
    }
}

// XDG helpers
pub fn xdg_config_home() -> PathBuf {
    env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".config"))
}

pub fn xdg_data_home() -> PathBuf {
    env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".local/share"))
}

pub fn xdg_cache_home() -> PathBuf {
    env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".cache"))
}

fn dirs_home() -> PathBuf {
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Find the repo/install directory where config files are located
pub fn find_config_dir() -> Result<PathBuf> {
    // 1. Check HYPRWHSPR_INSTALL_DIR env var
    if let Ok(dir) = env::var("HYPRWHSPR_INSTALL_DIR") {
        let path = PathBuf::from(dir);
        if path.join("config").exists() {
            return Ok(path);
        }
    }

    // 2. Relative to executable
    if let Ok(exe_path) = env::current_exe() {
        // Check ../share/hyprwhspr-rs/ (installed layout)
        if let Some(share_path) = exe_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("share/hyprwhspr-rs"))
        {
            if share_path.join("config").exists() {
                return Ok(share_path);
            }
        }

        // Check parent dir (dev layout: target/release/../..)
        if let Some(dev_path) = exe_path.parent().and_then(|p| p.parent()).and_then(|p| p.parent())
        {
            if dev_path.join("config").exists() {
                return Ok(dev_path.to_path_buf());
            }
        }
    }

    // 3. Check CARGO_MANIFEST_DIR (cargo run)
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let path = PathBuf::from(manifest_dir);
        if path.join("config").exists() {
            return Ok(path);
        }
    }

    // 4. Current working directory
    let cwd = env::current_dir()?;
    if cwd.join("config").exists() {
        return Ok(cwd);
    }

    // 5. XDG data dir
    let xdg_path = xdg_data_home().join("hyprwhspr-rs");
    if xdg_path.join("config").exists() {
        return Ok(xdg_path);
    }

    anyhow::bail!(
        "Could not find config directory. Make sure you're running from the hyprwhspr-rs directory \
         or the config files are installed."
    )
}

/// Backup a file before modification
pub fn backup_file(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let timestamp = OffsetDateTime::now_utc();
    let backup_name = format!(
        "{}.backup-{:04}{:02}{:02}_{:02}{:02}{:02}",
        path.file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
        timestamp.year(),
        timestamp.month() as u8,
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    );

    let backup_path = path.with_file_name(backup_name);
    fs::copy(path, &backup_path)?;
    println!("  {} Backup: {}", "○".blue(), backup_path.display());
    Ok(Some(backup_path))
}

/// Copy a file, prompting for overwrite if it exists (unless force=true)
pub fn copy_with_prompt(src: &Path, dst: &Path, force: bool) -> Result<CopyResult> {
    if dst.exists() && !force {
        if io::stdin().is_terminal() {
            let overwrite = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("{} exists. Overwrite?", dst.display()))
                .default(false)
                .interact()?;

            if !overwrite {
                println!("  {} Skipped: {}", "○".yellow(), dst.display());
                return Ok(CopyResult::Skipped);
            }
        } else {
            println!("  {} Skipped (exists): {}", "○".yellow(), dst.display());
            return Ok(CopyResult::Skipped);
        }

        backup_file(dst)?;
        fs::copy(src, dst)?;
        println!("  {} Overwritten: {}", "✓".green(), dst.display());
        Ok(CopyResult::Overwritten)
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)?;
        println!("  {} Copied: {}", "✓".green(), dst.display());
        Ok(CopyResult::Copied)
    }
}
