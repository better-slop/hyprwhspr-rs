use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hyprwhspr-rs", version, about = "Native speech-to-text voice dictation for Hyprland")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Run in test mode (Enter to toggle recording)
    #[arg(long)]
    pub test: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Install integration components (waybar, systemd, elephant)
    Install(InstallArgs),
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Install Waybar module and CSS
    #[arg(long)]
    pub waybar: bool,

    /// Install systemd user service
    #[arg(long)]
    pub service: bool,

    /// Install Elephant menu for Walker
    #[arg(long)]
    pub elephant: bool,

    /// Install all components (non-interactive)
    #[arg(long)]
    pub all: bool,

    /// Overwrite existing files without prompting
    #[arg(long, short)]
    pub force: bool,
}

impl InstallArgs {
    /// Returns true if any specific component flag was set
    pub fn has_specific_flags(&self) -> bool {
        self.waybar || self.service || self.elephant || self.all
    }
}
