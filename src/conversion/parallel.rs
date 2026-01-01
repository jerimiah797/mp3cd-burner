//! Parallel audio conversion using tokio
//!
//! Converts multiple audio files concurrently using a worker pool
//! sized based on CPU cores.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::ConversionResult;

/// Calculate the optimal number of parallel workers based on CPU cores
pub fn calculate_worker_count() -> usize {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Use 75% of cores, clamped between 2 and 8
    ((available as f32 * 0.75).ceil() as usize).clamp(2, 8)
}

/// Progress tracking for conversion
#[derive(Debug)]
pub struct ConversionProgress {
    pub completed: AtomicUsize,
    pub failed: AtomicUsize,
    pub total: usize,
}

impl ConversionProgress {
    pub fn new(total: usize) -> Self {
        Self {
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            total,
        }
    }

    pub fn increment_completed(&self) -> usize {
        self.completed.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn increment_failed(&self) -> usize {
        self.failed.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn completed_count(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    pub fn failed_count(&self) -> usize {
        self.failed.load(Ordering::SeqCst)
    }
}

/// A file to be converted
#[derive(Debug, Clone)]
pub struct ConversionJob {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
}

/// Convert a single file asynchronously
async fn convert_file_async(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    bitrate: u32,
) -> ConversionResult {
    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: false,
                    error: Some(format!("Failed to create output directory: {}", e)),
                };
            }
        }
    }

    let bitrate_str = format!("{}k", bitrate);

    let result = Command::new(ffmpeg_path)
        .arg("-i")
        .arg(input_path)
        .arg("-vn")
        .arg("-codec:a")
        .arg("libmp3lame")
        .arg("-b:a")
        .arg(&bitrate_str)
        .arg("-y")
        .arg(output_path)
        .output()
        .await;

    match result {
        Ok(output) => {
            if output.status.success() {
                ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: true,
                    error: None,
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let error_msg = format!(
                    "ffmpeg exited with status {}: {}",
                    output.status,
                    stderr.lines().last().unwrap_or("Unknown error")
                );
                ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: false,
                    error: Some(error_msg),
                }
            }
        }
        Err(e) => ConversionResult {
            output_path: output_path.to_path_buf(),
            input_path: input_path.to_path_buf(),
            success: false,
            error: Some(format!("Failed to spawn ffmpeg: {}", e)),
        },
    }
}

/// Convert multiple files in parallel
///
/// Returns the total number of successful and failed conversions
pub async fn convert_files_parallel(
    ffmpeg_path: PathBuf,
    jobs: Vec<ConversionJob>,
    bitrate: u32,
    progress: Arc<ConversionProgress>,
) -> (usize, usize) {
    convert_files_parallel_with_callback(ffmpeg_path, jobs, bitrate, progress, || {}).await
}

/// Convert multiple files in parallel with a callback after each file completes
///
/// The `on_file_complete` callback is called on the tokio runtime thread
/// after each file finishes (success or failure). This can be used to
/// trigger UI updates.
pub async fn convert_files_parallel_with_callback<F>(
    ffmpeg_path: PathBuf,
    jobs: Vec<ConversionJob>,
    bitrate: u32,
    progress: Arc<ConversionProgress>,
    on_file_complete: F,
) -> (usize, usize)
where
    F: Fn() + Send + Sync + 'static,
{
    let worker_count = calculate_worker_count();
    let semaphore = Arc::new(Semaphore::new(worker_count));
    let on_complete = Arc::new(on_file_complete);

    println!(
        "Starting parallel conversion: {} files with {} workers",
        jobs.len(),
        worker_count
    );

    // Use FuturesUnordered to process completions as they happen
    let mut futures = FuturesUnordered::new();

    for job in jobs {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let ffmpeg = ffmpeg_path.clone();
        let progress = progress.clone();
        let on_complete = on_complete.clone();

        let handle = tokio::spawn(async move {
            let input_name = job.input_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            println!("Converting: {}", input_name);

            let result = convert_file_async(
                &ffmpeg,
                &job.input_path,
                &job.output_path,
                bitrate,
            )
            .await;

            if result.success {
                let count = progress.increment_completed();
                println!(
                    "Completed ({}/{}): {}",
                    count,
                    progress.total,
                    input_name
                );
            } else {
                progress.increment_failed();
                if let Some(ref error) = result.error {
                    eprintln!("Failed: {} - {}", input_name, error);
                }
            }

            // Call callback immediately when this file completes
            on_complete();

            drop(permit); // Release the semaphore permit
            result
        });

        futures.push(handle);
    }

    // Wait for all tasks to complete
    while let Some(_result) = futures.next().await {
        // Tasks already called on_complete when they finished
    }

    (progress.completed_count(), progress.failed_count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_worker_count() {
        let count = calculate_worker_count();
        assert!(count >= 2 && count <= 8);
    }

    #[test]
    fn test_conversion_progress() {
        let progress = ConversionProgress::new(10);
        assert_eq!(progress.completed_count(), 0);
        assert_eq!(progress.failed_count(), 0);

        progress.increment_completed();
        progress.increment_completed();
        progress.increment_failed();

        assert_eq!(progress.completed_count(), 2);
        assert_eq!(progress.failed_count(), 1);
    }
}
