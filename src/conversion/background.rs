//! Background encoder for immediate folder conversion
//!
//! This module provides a background encoding system that converts folders
//! as soon as they're added to the list, enabling:
//! - Immediate encoding without waiting for "Burn" button
//! - Per-folder progress tracking
//! - Smart cancellation when folders are removed
//! - Bitrate recalculation triggers

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::output_manager::OutputManager;
use super::parallel::{convert_files_parallel_with_callback, ConversionJob, ConversionProgress};
use super::{get_ffmpeg_path, verify_ffmpeg};
use crate::audio::{determine_encoding_strategy, EncodingStrategy};
use crate::core::{FolderConversionStatus, FolderId, MusicFolder};

/// Commands that can be sent to the background encoder
#[derive(Debug)]
pub enum EncoderCommand {
    /// Add a folder to be encoded
    AddFolder(FolderId, MusicFolder),
    /// Remove a folder (cancel if active, remove from queue)
    RemoveFolder(FolderId),
    /// Notify that folders were reordered (no re-encoding needed)
    FoldersReordered,
    /// Recalculate lossless bitrate with new target
    RecalculateBitrate { target_bitrate: u32 },
    /// Shutdown the encoder
    Shutdown,
}

/// Events emitted by the background encoder
#[derive(Debug, Clone)]
pub enum EncoderEvent {
    /// Started encoding a folder
    FolderStarted {
        id: FolderId,
        files_total: usize,
    },
    /// Progress update for a folder
    FolderProgress {
        id: FolderId,
        files_completed: usize,
        files_total: usize,
    },
    /// Folder encoding completed successfully
    FolderCompleted {
        id: FolderId,
        output_dir: PathBuf,
        output_size: u64,
        lossless_bitrate: Option<u32>,
    },
    /// Folder encoding failed
    FolderFailed {
        id: FolderId,
        error: String,
    },
    /// Folder was cancelled (removed mid-encoding)
    FolderCancelled(FolderId),
    /// Bitrate was recalculated, some folders need re-encoding
    BitrateRecalculated {
        new_bitrate: u32,
        reencode_needed: Vec<FolderId>,
    },
}

/// File category for conversion ordering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    /// MP3 files - just copy
    Mp3Copy,
    /// Lossy non-MP3 - transcode at source bitrate
    Lossy,
    /// Lossless - transcode at target bitrate
    Lossless,
}

/// Information about a file to be encoded
#[derive(Debug, Clone)]
pub struct FileToEncode {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub category: FileCategory,
    pub strategy: EncodingStrategy,
}

/// Per-folder encoding task state
#[derive(Debug)]
struct FolderTask {
    folder: MusicFolder,
    cancel_token: Arc<AtomicBool>,
}

/// Shared state for the background encoder
pub struct BackgroundEncoderState {
    /// Queue of folders waiting to be encoded
    pub queue: VecDeque<(FolderId, MusicFolder)>,
    /// Currently active folder (if any)
    pub active: Option<(FolderId, Arc<AtomicBool>)>,
    /// Completed folders with their status
    pub completed: HashMap<FolderId, FolderConversionStatus>,
    /// Current lossless bitrate target
    pub lossless_bitrate: u32,
    /// Whether the encoder is running
    pub is_running: bool,
}

impl BackgroundEncoderState {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            active: None,
            completed: HashMap::new(),
            lossless_bitrate: 320, // Default to max quality
            is_running: false,
        }
    }

    /// Get the total number of pending and active folders
    pub fn pending_count(&self) -> usize {
        self.queue.len() + if self.active.is_some() { 1 } else { 0 }
    }

    /// Check if a folder is queued or active
    pub fn is_pending(&self, id: &FolderId) -> bool {
        self.queue.iter().any(|(fid, _)| fid == id)
            || self.active.as_ref().map(|(fid, _)| fid == id).unwrap_or(false)
    }

    /// Check if a folder is completed
    pub fn is_completed(&self, id: &FolderId) -> bool {
        self.completed.contains_key(id)
    }
}

impl Default for BackgroundEncoderState {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to the background encoder for sending commands
#[derive(Clone)]
pub struct BackgroundEncoderHandle {
    command_tx: mpsc::UnboundedSender<EncoderCommand>,
    pub state: Arc<Mutex<BackgroundEncoderState>>,
}

impl BackgroundEncoderHandle {
    /// Add a folder to be encoded
    pub fn add_folder(&self, folder: MusicFolder) {
        let id = folder.id.clone();
        let _ = self.command_tx.send(EncoderCommand::AddFolder(id, folder));
    }

    /// Remove a folder (cancel if active)
    pub fn remove_folder(&self, id: &FolderId) {
        let _ = self.command_tx.send(EncoderCommand::RemoveFolder(id.clone()));
    }

    /// Notify that folders were reordered
    pub fn folders_reordered(&self) {
        let _ = self.command_tx.send(EncoderCommand::FoldersReordered);
    }

    /// Request bitrate recalculation
    pub fn recalculate_bitrate(&self, target: u32) {
        let _ = self
            .command_tx
            .send(EncoderCommand::RecalculateBitrate { target_bitrate: target });
    }

    /// Shutdown the encoder
    pub fn shutdown(&self) {
        let _ = self.command_tx.send(EncoderCommand::Shutdown);
    }

    /// Get the current state (for reading status)
    pub fn get_state(&self) -> Arc<Mutex<BackgroundEncoderState>> {
        self.state.clone()
    }
}

/// The background encoder that runs conversions
pub struct BackgroundEncoder {
    output_manager: OutputManager,
    state: Arc<Mutex<BackgroundEncoderState>>,
    ffmpeg_path: PathBuf,
}

impl BackgroundEncoder {
    /// Create a new background encoder
    pub fn new() -> Result<(Self, BackgroundEncoderHandle, mpsc::UnboundedReceiver<EncoderEvent>), String> {
        let output_manager = OutputManager::new()?;
        let ffmpeg_path = verify_ffmpeg()?;
        let state = Arc::new(Mutex::new(BackgroundEncoderState::new()));

        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let encoder = Self {
            output_manager,
            state: state.clone(),
            ffmpeg_path,
        };

        let handle = BackgroundEncoderHandle {
            command_tx,
            state: state.clone(),
        };

        // Start the encoder task
        let state_clone = state.clone();
        let output_manager_clone = encoder.output_manager.clone();
        let ffmpeg_path_clone = encoder.ffmpeg_path.clone();

        tokio::spawn(async move {
            run_encoder_loop(
                state_clone,
                output_manager_clone,
                ffmpeg_path_clone,
                command_rx,
                event_tx,
            )
            .await;
        });

        Ok((encoder, handle, event_rx))
    }

    /// Get a reference to the output manager
    pub fn output_manager(&self) -> &OutputManager {
        &self.output_manager
    }
}

/// Main encoder loop - processes commands and runs conversions
async fn run_encoder_loop(
    state: Arc<Mutex<BackgroundEncoderState>>,
    output_manager: OutputManager,
    ffmpeg_path: PathBuf,
    mut command_rx: mpsc::UnboundedReceiver<EncoderCommand>,
    event_tx: mpsc::UnboundedSender<EncoderEvent>,
) {
    state.lock().unwrap().is_running = true;
    println!("Background encoder started");

    loop {
        // Process any pending commands first
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                EncoderCommand::Shutdown => {
                    println!("Background encoder shutting down");
                    state.lock().unwrap().is_running = false;
                    return;
                }
                EncoderCommand::AddFolder(id, folder) => {
                    let mut s = state.lock().unwrap();
                    // Don't add if already queued/active/completed
                    if !s.is_pending(&id) && !s.is_completed(&id) {
                        println!("Queued folder for encoding: {}", folder.path.display());
                        s.queue.push_back((id, folder));
                    }
                }
                EncoderCommand::RemoveFolder(id) => {
                    let mut s = state.lock().unwrap();
                    // Remove from queue if present
                    s.queue.retain(|(fid, _)| fid != &id);
                    // Cancel if active
                    if let Some((active_id, cancel_token)) = &s.active {
                        if active_id == &id {
                            cancel_token.store(true, Ordering::SeqCst);
                        }
                    }
                    // Remove from completed
                    s.completed.remove(&id);
                    // Clean up output directory
                    drop(s); // Release lock before file operations
                    let _ = output_manager.delete_folder_output(&id);
                    let _ = event_tx.send(EncoderEvent::FolderCancelled(id));
                }
                EncoderCommand::FoldersReordered => {
                    // No action needed - ISO staging handles reordering
                    println!("Folders reordered - ISO will be regenerated");
                }
                EncoderCommand::RecalculateBitrate { target_bitrate } => {
                    let mut s = state.lock().unwrap();
                    let old_bitrate = s.lossless_bitrate;
                    s.lossless_bitrate = target_bitrate;

                    if old_bitrate != target_bitrate {
                        // Find folders that need re-encoding
                        let mut reencode_needed = Vec::new();
                        for (id, status) in &s.completed {
                            if let FolderConversionStatus::Converted {
                                lossless_bitrate: Some(br),
                                ..
                            } = status
                            {
                                if *br != target_bitrate {
                                    reencode_needed.push(id.clone());
                                }
                            }
                        }

                        if !reencode_needed.is_empty() {
                            let _ = event_tx.send(EncoderEvent::BitrateRecalculated {
                                new_bitrate: target_bitrate,
                                reencode_needed,
                            });
                        }
                    }
                }
            }
        }

        // Check if there's work to do
        let next_folder = {
            let mut s = state.lock().unwrap();
            if s.active.is_none() {
                s.queue.pop_front()
            } else {
                None
            }
        };

        if let Some((id, folder)) = next_folder {
            // Start encoding this folder
            let cancel_token = Arc::new(AtomicBool::new(false));
            {
                let mut s = state.lock().unwrap();
                s.active = Some((id.clone(), cancel_token.clone()));
            }

            let files_total = folder.audio_files.len();
            let _ = event_tx.send(EncoderEvent::FolderStarted {
                id: id.clone(),
                files_total,
            });

            // Get output directory for this folder
            let output_dir = match output_manager.get_folder_output_dir(&id) {
                Ok(dir) => dir,
                Err(e) => {
                    let _ = event_tx.send(EncoderEvent::FolderFailed {
                        id: id.clone(),
                        error: e,
                    });
                    state.lock().unwrap().active = None;
                    continue;
                }
            };

            // Get current lossless bitrate
            let lossless_bitrate = state.lock().unwrap().lossless_bitrate;

            // Build conversion jobs
            let mut jobs = Vec::new();
            let mut has_lossless = false;

            for file in &folder.audio_files {
                // Parameters: codec, source_bitrate, target_bitrate, is_lossy, no_lossy_mode, embed_album_art
                // For CD burning, we don't embed album art (saves space)
                let strategy = determine_encoding_strategy(
                    &file.codec,
                    file.bitrate,
                    lossless_bitrate,
                    file.is_lossy,
                    false, // no_lossy_mode - allow lossy conversions
                    false, // embed_album_art - strip for CD burning
                );

                if matches!(strategy, EncodingStrategy::ConvertAtTargetBitrate(_)) {
                    has_lossless = true;
                }

                let output_name = file
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                let output_path = output_dir.join(format!("{}.mp3", output_name));

                jobs.push(ConversionJob {
                    input_path: file.path.clone(),
                    output_path,
                    strategy,
                });
            }

            // Sort jobs: Copy first, then lossy, then lossless
            jobs.sort_by_key(|j| match &j.strategy {
                EncodingStrategy::Copy => 0,
                EncodingStrategy::CopyWithoutArt => 1,
                EncodingStrategy::ConvertAtSourceBitrate(_) => 2,
                EncodingStrategy::ConvertAtTargetBitrate(_) => 3,
            });

            // Run conversion
            let progress = Arc::new(ConversionProgress::new(jobs.len()));
            let event_tx_clone = event_tx.clone();
            let id_clone = id.clone();
            let progress_clone = progress.clone();

            let (completed, _failed, was_cancelled) = convert_files_parallel_with_callback(
                ffmpeg_path.clone(),
                jobs,
                progress.clone(),
                cancel_token.clone(),
                move || {
                    let completed = progress_clone.completed_count();
                    let total = progress_clone.total;
                    let _ = event_tx_clone.send(EncoderEvent::FolderProgress {
                        id: id_clone.clone(),
                        files_completed: completed,
                        files_total: total,
                    });
                },
            )
            .await;

            // Update state
            {
                let mut s = state.lock().unwrap();
                s.active = None;

                if was_cancelled || cancel_token.load(Ordering::SeqCst) {
                    // Was cancelled - don't mark as completed
                    let _ = event_tx.send(EncoderEvent::FolderCancelled(id.clone()));
                } else if completed == files_total {
                    // Success!
                    let output_size = output_manager
                        .get_folder_output_size(&id)
                        .unwrap_or(0);

                    let status = FolderConversionStatus::Converted {
                        output_dir: output_dir.clone(),
                        lossless_bitrate: if has_lossless { Some(lossless_bitrate) } else { None },
                        output_size,
                        completed_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    };

                    s.completed.insert(id.clone(), status);

                    let _ = event_tx.send(EncoderEvent::FolderCompleted {
                        id: id.clone(),
                        output_dir,
                        output_size,
                        lossless_bitrate: if has_lossless { Some(lossless_bitrate) } else { None },
                    });
                } else {
                    // Some files failed
                    let _ = event_tx.send(EncoderEvent::FolderFailed {
                        id: id.clone(),
                        error: format!(
                            "Only {} of {} files converted successfully",
                            completed, files_total
                        ),
                    });
                }
            }
        } else {
            // No work to do - wait a bit before checking again
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        // Also check for incoming commands during sleep
        tokio::task::yield_now().await;
    }
}

/// Calculate the optimal lossless bitrate given capacity constraints
pub fn calculate_optimal_lossless_bitrate(
    lossless_duration: f64,
    available_bytes: u64,
) -> u32 {
    if lossless_duration <= 0.0 {
        return 320; // No lossless files, use max
    }

    // Calculate what bitrate would fit
    // bitrate (kbps) = (bytes * 8) / (duration * 1000)
    let available_kbps = (available_bytes as f64 * 8.0) / (lossless_duration * 1000.0);

    // Clamp to valid MP3 bitrates
    let bitrate = available_kbps.floor() as u32;

    // Round down to nearest standard bitrate
    match bitrate {
        0..=95 => 64,
        96..=127 => 96,
        128..=159 => 128,
        160..=191 => 160,
        192..=223 => 192,
        224..=255 => 224,
        256..=319 => 256,
        _ => 320,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_category_ordering() {
        assert!(FileCategory::Mp3Copy as u8 == 0);
        assert!(FileCategory::Lossy as u8 == 1);
        assert!(FileCategory::Lossless as u8 == 2);
    }

    #[test]
    fn test_background_encoder_state_new() {
        let state = BackgroundEncoderState::new();
        assert_eq!(state.queue.len(), 0);
        assert!(state.active.is_none());
        assert_eq!(state.completed.len(), 0);
        assert_eq!(state.lossless_bitrate, 320);
        assert!(!state.is_running);
    }

    #[test]
    fn test_background_encoder_state_pending_count() {
        let mut state = BackgroundEncoderState::new();
        assert_eq!(state.pending_count(), 0);

        // Add to queue
        let folder_id = FolderId("test1".to_string());
        let folder = create_test_folder("test1");
        state.queue.push_back((folder_id, folder));
        assert_eq!(state.pending_count(), 1);

        // Set active
        let folder_id2 = FolderId("test2".to_string());
        state.active = Some((folder_id2, Arc::new(AtomicBool::new(false))));
        assert_eq!(state.pending_count(), 2);
    }

    #[test]
    fn test_calculate_optimal_lossless_bitrate() {
        // No duration - max bitrate
        assert_eq!(calculate_optimal_lossless_bitrate(0.0, 1000000), 320);

        // Large capacity - max bitrate
        let huge_capacity = 700 * 1024 * 1024; // 700 MB
        assert_eq!(calculate_optimal_lossless_bitrate(3600.0, huge_capacity), 320);

        // Limited capacity
        // 60 seconds of audio, 1MB available
        // bitrate = (1000000 * 8) / (60 * 1000) = 133 kbps -> rounds to 128
        assert_eq!(calculate_optimal_lossless_bitrate(60.0, 1000000), 128);

        // Very limited
        // 60 seconds, 500KB = 66 kbps -> rounds to 64
        assert_eq!(calculate_optimal_lossless_bitrate(60.0, 500000), 64);
    }

    #[test]
    fn test_is_pending() {
        let mut state = BackgroundEncoderState::new();
        let id1 = FolderId("folder1".to_string());
        let id2 = FolderId("folder2".to_string());
        let id3 = FolderId("folder3".to_string());

        assert!(!state.is_pending(&id1));

        // Add to queue
        state.queue.push_back((id1.clone(), create_test_folder("test")));
        assert!(state.is_pending(&id1));
        assert!(!state.is_pending(&id2));

        // Set active
        state.active = Some((id2.clone(), Arc::new(AtomicBool::new(false))));
        assert!(state.is_pending(&id2));
        assert!(!state.is_pending(&id3));
    }

    #[test]
    fn test_is_completed() {
        let mut state = BackgroundEncoderState::new();
        let id = FolderId("completed_folder".to_string());

        assert!(!state.is_completed(&id));

        state.completed.insert(
            id.clone(),
            FolderConversionStatus::Converted {
                output_dir: PathBuf::from("/tmp/test"),
                lossless_bitrate: Some(320),
                output_size: 1000,
                completed_at: 0,
            },
        );

        assert!(state.is_completed(&id));
    }

    // Helper to create a minimal MusicFolder for testing
    fn create_test_folder(name: &str) -> MusicFolder {
        MusicFolder {
            id: FolderId(name.to_string()),
            path: PathBuf::from(format!("/test/{}", name)),
            file_count: 0,
            total_size: 0,
            total_duration: 0.0,
            album_art: None,
            audio_files: vec![],
            conversion_status: FolderConversionStatus::default(),
        }
    }
}
