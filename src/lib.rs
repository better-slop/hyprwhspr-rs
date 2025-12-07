pub mod app;
pub mod app_test;
pub mod audio;
pub mod benchmark;
pub mod cli;
pub mod config;
pub mod input;
pub mod install;
pub mod logging;
pub mod paths;
pub mod status;
pub mod transcription;
pub mod whisper;

pub use app::HyprwhsprApp;
pub use config::{Config, ConfigManager};
pub use status::StatusWriter;
