//! Parallel audio conversion using tokio
//!
//! Converts multiple audio files concurrently using a worker pool
//! sized based on CPU cores.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::ConversionResult;
use crate::audio::EncodingStrategy;

/// Calculate the optimal number of parallel workers based on CPU cores
fn calculate_worker_count() -> usize {
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
    pub strategy: EncodingStrategy,
}

/// Convert a single file asynchronously based on encoding strategy
async fn convert_file_async(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    strategy: &EncodingStrategy,
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

    match strategy {
        EncodingStrategy::Copy => {
            // Direct file copy - no ffmpeg needed, fastest option
            match tokio::fs::copy(input_path, output_path).await {
                Ok(_) => ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: true,
                    error: None,
                },
                Err(e) => ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: false,
                    error: Some(format!("Failed to copy file: {}", e)),
                },
            }
        }
        EncodingStrategy::CopyWithoutArt => {
            // Copy audio stream without re-encoding, strip album art
            let result = Command::new(ffmpeg_path)
                .arg("-i")
                .arg(input_path)
                .arg("-vn")           // Skip video/album art
                .arg("-codec:a")
                .arg("copy")          // Copy audio stream as-is
                .arg("-y")
                .arg(output_path)
                .output()
                .await;

            handle_ffmpeg_result(result, input_path, output_path)
        }
        EncodingStrategy::ConvertAtSourceBitrate(bitrate) => {
            // Transcode lossy source to MP3 at specified bitrate (ABR mode)
            let bitrate_str = format!("{}k", bitrate);

            let result = Command::new(ffmpeg_path)
                .arg("-i")
                .arg(input_path)
                .arg("-vn")           // Skip video/album art for CD burning
                .arg("-codec:a")
                .arg("libmp3lame")
                .arg("-b:a")
                .arg(&bitrate_str)
                .arg("-y")
                .arg(output_path)
                .output()
                .await;

            handle_ffmpeg_result(result, input_path, output_path)
        }
        EncodingStrategy::ConvertAtTargetBitrate(bitrate) => {
            // Transcode lossless to MP3 at specified bitrate (CBR mode for predictable size)
            let bitrate_str = format!("{}k", bitrate);

            let result = Command::new(ffmpeg_path)
                .arg("-i")
                .arg(input_path)
                .arg("-vn")           // Skip video/album art for CD burning
                .arg("-codec:a")
                .arg("libmp3lame")
                .arg("-abr")
                .arg("0")             // Force CBR mode for predictable file size
                .arg("-b:a")
                .arg(&bitrate_str)
                .arg("-y")
                .arg(output_path)
                .output()
                .await;

            handle_ffmpeg_result(result, input_path, output_path)
        }
    }
}

/// Handle ffmpeg command result and convert to ConversionResult
fn handle_ffmpeg_result(
    result: Result<std::process::Output, std::io::Error>,
    input_path: &Path,
    output_path: &Path,
) -> ConversionResult {
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

/// Convert multiple files in parallel with a callback after each file completes
///
/// The `on_file_complete` callback is called on the tokio runtime thread
/// after each file finishes (success or failure). This can be used to
/// trigger UI updates.
///
/// The `cancel_token` can be set to true to stop processing new files.
/// Files that are already in progress will complete, but no new files will start.
pub async fn convert_files_parallel_with_callback<F>(
    ffmpeg_path: PathBuf,
    jobs: Vec<ConversionJob>,
    progress: Arc<ConversionProgress>,
    cancel_token: Arc<AtomicBool>,
    on_file_complete: F,
) -> (usize, usize, bool)
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
    let mut was_cancelled = false;

    for job in jobs {
        // Check for cancellation before starting each new job
        if cancel_token.load(Ordering::SeqCst) {
            println!("Cancellation requested - skipping remaining files");
            was_cancelled = true;
            break;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let ffmpeg = ffmpeg_path.clone();
        let progress = progress.clone();
        let on_complete = on_complete.clone();

        let handle = tokio::spawn(async move {
            let input_name = job.input_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            // Log the strategy being used
            let strategy_desc = match &job.strategy {
                EncodingStrategy::Copy => "copy".to_string(),
                EncodingStrategy::CopyWithoutArt => "copy (no art)".to_string(),
                EncodingStrategy::ConvertAtSourceBitrate(br) => format!("transcode @{}k (source)", br),
                EncodingStrategy::ConvertAtTargetBitrate(br) => format!("transcode @{}k (target)", br),
            };
            println!("Processing: {} [{}]", input_name, strategy_desc);

            let result = convert_file_async(
                &ffmpeg,
                &job.input_path,
                &job.output_path,
                &job.strategy,
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

    // Wait for all in-flight tasks to complete (even if cancelled)
    while let Some(_result) = futures.next().await {
        // Tasks already called on_complete when they finished
    }

    (progress.completed_count(), progress.failed_count(), was_cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

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

    #[test]
    fn test_conversion_progress_increment_returns_new_count() {
        let progress = ConversionProgress::new(5);

        assert_eq!(progress.increment_completed(), 1);
        assert_eq!(progress.increment_completed(), 2);
        assert_eq!(progress.increment_failed(), 1);
        assert_eq!(progress.increment_failed(), 2);
    }

    #[test]
    fn test_conversion_job_creation() {
        let job = ConversionJob {
            input_path: PathBuf::from("/input/song.flac"),
            output_path: PathBuf::from("/output/song.mp3"),
            strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
        };

        assert_eq!(job.input_path, PathBuf::from("/input/song.flac"));
        assert_eq!(job.output_path, PathBuf::from("/output/song.mp3"));
    }

    #[tokio::test]
    async fn test_parallel_conversion_empty_jobs() {
        let ffmpeg_path = PathBuf::from("/nonexistent/ffmpeg");
        let jobs: Vec<ConversionJob> = vec![];
        let progress = Arc::new(ConversionProgress::new(0));
        let cancel_token = Arc::new(AtomicBool::new(false));
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let (completed, failed, cancelled) = convert_files_parallel_with_callback(
            ffmpeg_path,
            jobs,
            progress,
            cancel_token,
            move || {
                callback_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;

        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert!(!cancelled);
        assert_eq!(callback_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_parallel_conversion_callback_invoked_per_file() {
        // Use a nonexistent ffmpeg - files will fail but callbacks should fire
        let ffmpeg_path = PathBuf::from("/nonexistent/ffmpeg");
        let jobs = vec![
            ConversionJob {
                input_path: PathBuf::from("/fake/1.flac"),
                output_path: PathBuf::from("/tmp/1.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
            ConversionJob {
                input_path: PathBuf::from("/fake/2.flac"),
                output_path: PathBuf::from("/tmp/2.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
            ConversionJob {
                input_path: PathBuf::from("/fake/3.flac"),
                output_path: PathBuf::from("/tmp/3.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
        ];
        let progress = Arc::new(ConversionProgress::new(3));
        let cancel_token = Arc::new(AtomicBool::new(false));
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let (completed, failed, cancelled) = convert_files_parallel_with_callback(
            ffmpeg_path,
            jobs,
            progress.clone(),
            cancel_token,
            move || {
                callback_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;

        // All should fail (no ffmpeg), but callbacks should fire for each
        assert_eq!(completed, 0);
        assert_eq!(failed, 3);
        assert!(!cancelled);
        assert_eq!(callback_count.load(Ordering::SeqCst), 3);
        assert_eq!(progress.failed_count(), 3);
    }

    #[tokio::test]
    async fn test_parallel_conversion_progress_tracking() {
        let ffmpeg_path = PathBuf::from("/nonexistent/ffmpeg");
        let jobs = vec![
            ConversionJob {
                input_path: PathBuf::from("/fake/a.flac"),
                output_path: PathBuf::from("/tmp/a.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
            ConversionJob {
                input_path: PathBuf::from("/fake/b.flac"),
                output_path: PathBuf::from("/tmp/b.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
        ];
        let progress = Arc::new(ConversionProgress::new(2));
        let cancel_token = Arc::new(AtomicBool::new(false));

        let (completed, failed, _cancelled) = convert_files_parallel_with_callback(
            ffmpeg_path,
            jobs,
            progress.clone(),
            cancel_token,
            || {},
        )
        .await;

        // Verify progress struct matches return values
        assert_eq!(progress.completed_count(), completed);
        assert_eq!(progress.failed_count(), failed);
        assert_eq!(progress.total, 2);
    }

    #[tokio::test]
    async fn test_parallel_conversion_cancellation() {
        let ffmpeg_path = PathBuf::from("/nonexistent/ffmpeg");
        let jobs = vec![
            ConversionJob {
                input_path: PathBuf::from("/fake/1.flac"),
                output_path: PathBuf::from("/tmp/1.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
            ConversionJob {
                input_path: PathBuf::from("/fake/2.flac"),
                output_path: PathBuf::from("/tmp/2.mp3"),
                strategy: EncodingStrategy::ConvertAtTargetBitrate(256),
            },
        ];
        let progress = Arc::new(ConversionProgress::new(2));
        // Pre-cancel before starting
        let cancel_token = Arc::new(AtomicBool::new(true));

        let (completed, failed, cancelled) = convert_files_parallel_with_callback(
            ffmpeg_path,
            jobs,
            progress.clone(),
            cancel_token,
            || {},
        )
        .await;

        // Should have been cancelled immediately, no jobs processed
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert!(cancelled);
    }
}
