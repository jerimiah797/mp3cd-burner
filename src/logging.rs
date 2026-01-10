//! Logging configuration for MP3 CD Burner
//!
//! Logs are written to both the terminal and a file at:
//! `~/Library/Logs/MP3-CD-Burner/mp3cd-burner.log`
//!
//! Users can find this log file to send for debugging purposes.

use log::LevelFilter;
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, SharedLogger, TermLogger, TerminalMode,
    WriteLogger,
};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;

/// Get the log directory path
/// On macOS: ~/Library/Logs/MP3-CD-Burner/
pub fn get_log_directory() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join("Library").join("Logs").join("MP3-CD-Burner"))
    } else {
        // Fallback for other platforms
        dirs::data_local_dir().map(|d| d.join("MP3-CD-Burner").join("logs"))
    }
}

/// Get the current log file path
pub fn get_log_file_path() -> Option<PathBuf> {
    get_log_directory().map(|d| d.join("mp3cd-burner.log"))
}

/// Initialize the logging system
///
/// Sets up combined logging to:
/// - Terminal (with colors, for development)
/// - File (for user bug reports)
///
/// Returns the path to the log file on success
pub fn init_logging() -> Option<PathBuf> {
    let log_dir = match get_log_directory() {
        Some(d) => d,
        None => {
            eprintln!("Warning: Could not determine log directory");
            return None;
        }
    };

    // Create log directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(&log_dir) {
        eprintln!("Warning: Could not create log directory: {}", e);
        return None;
    }

    let log_path = log_dir.join("mp3cd-burner.log");

    // Rotate old log if it's too large (> 10MB)
    if let Ok(metadata) = fs::metadata(&log_path) {
        if metadata.len() > 10 * 1024 * 1024 {
            let backup_path = log_dir.join("mp3cd-burner.log.old");
            let _ = fs::rename(&log_path, &backup_path);
        }
    }

    // Open log file (append mode)
    let log_file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Warning: Could not open log file: {}", e);
            // Fall back to terminal-only logging
            init_terminal_only();
            return None;
        }
    };

    // Configure logging format
    let config = ConfigBuilder::new()
        .set_time_format_rfc3339()
        .set_thread_level(LevelFilter::Off) // Don't show thread IDs
        .set_target_level(LevelFilter::Off) // Don't show module targets
        .build();

    // Set up combined logger (terminal + file)
    let loggers: Vec<Box<dyn SharedLogger>> = vec![
        // Terminal logger - show info and above in terminal
        TermLogger::new(LevelFilter::Info, config.clone(), TerminalMode::Mixed, ColorChoice::Auto),
        // File logger - capture debug and above in file
        WriteLogger::new(LevelFilter::Debug, config, log_file),
    ];

    if CombinedLogger::init(loggers).is_err() {
        eprintln!("Warning: Logger already initialized");
    }

    // Write session start marker
    log::info!("=== MP3 CD Burner session started ===");
    log::info!("Log file: {}", log_path.display());

    Some(log_path)
}

/// Initialize terminal-only logging (fallback if file logging fails)
fn init_terminal_only() {
    let config = ConfigBuilder::new()
        .set_time_format_rfc3339()
        .set_thread_level(LevelFilter::Off)
        .set_target_level(LevelFilter::Off)
        .build();

    let term_logger = TermLogger::new(LevelFilter::Info, config, TerminalMode::Mixed, ColorChoice::Auto);
    let _ = CombinedLogger::init(vec![term_logger]);
}

/// Open the log directory in Finder (for users to access logs)
pub fn open_log_directory() -> Result<(), String> {
    if let Some(log_dir) = get_log_directory() {
        if log_dir.exists() {
            std::process::Command::new("open")
                .arg(&log_dir)
                .spawn()
                .map_err(|e| format!("Failed to open log directory: {}", e))?;
            Ok(())
        } else {
            Err("Log directory does not exist".to_string())
        }
    } else {
        Err("Could not determine log directory".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_log_directory_returns_path() {
        let dir = get_log_directory();
        assert!(dir.is_some(), "Should return a log directory path");

        let path = dir.unwrap();
        assert!(
            path.to_string_lossy().contains("MP3-CD-Burner"),
            "Path should contain app name"
        );
    }

    #[test]
    fn test_get_log_file_path_returns_path() {
        let path = get_log_file_path();
        assert!(path.is_some(), "Should return a log file path");

        let file_path = path.unwrap();
        assert!(
            file_path.to_string_lossy().ends_with("mp3cd-burner.log"),
            "Path should end with log filename"
        );
    }

    #[test]
    fn test_log_file_path_is_inside_log_directory() {
        let dir = get_log_directory().unwrap();
        let file = get_log_file_path().unwrap();

        assert!(
            file.starts_with(&dir),
            "Log file should be inside log directory"
        );
    }
}
