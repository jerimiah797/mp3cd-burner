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
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::output_manager::OutputManager;
use super::parallel::{convert_files_parallel_with_callback, ConversionJob, ConversionProgress};
use super::verify_ffmpeg;
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
    /// Clear all state (for New profile)
    ClearAll,
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

/// Shared state for the background encoder
pub struct BackgroundEncoderState {
    /// Queue of folders waiting to be encoded
    pub queue: VecDeque<(FolderId, MusicFolder)>,
    /// Currently active folder: (id, cancel_token, has_lossless_files)
    pub active: Option<(FolderId, Arc<AtomicBool>, bool)>,
    /// Progress of the active folder: (files_completed, files_total)
    pub active_progress: Option<(usize, usize)>,
    /// Completed folders with their status and original folder data (for re-encoding)
    pub completed: HashMap<FolderId, (FolderConversionStatus, MusicFolder)>,
    /// Current lossless bitrate target
    pub lossless_bitrate: u32,
    /// Whether the encoder is running
    pub is_running: bool,
    /// Folder ID that was cancelled due to bitrate change (needs re-queuing)
    pub pending_bitrate_requeue: Option<FolderId>,
}

impl BackgroundEncoderState {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            active: None,
            active_progress: None,
            completed: HashMap::new(),
            lossless_bitrate: 320, // Default to max quality
            is_running: false,
            pending_bitrate_requeue: None,
        }
    }

    /// Check if a folder is queued or active
    pub fn is_pending(&self, id: &FolderId) -> bool {
        self.queue.iter().any(|(fid, _)| fid == id)
            || self.active.as_ref().map(|(fid, _, _)| fid == id).unwrap_or(false)
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

    /// Clear all state (for New profile)
    pub fn clear_all(&self) {
        let _ = self.command_tx.send(EncoderCommand::ClearAll);
    }

    /// Get the current state (for reading status)
    pub fn get_state(&self) -> Arc<Mutex<BackgroundEncoderState>> {
        self.state.clone()
    }
}

/// The background encoder that runs conversions
///
/// Note: Fields are used internally during construction but not accessed
/// after new() returns - the handle is used instead for all operations.
#[allow(dead_code)]
pub struct BackgroundEncoder {
    output_manager: OutputManager,
    state: Arc<Mutex<BackgroundEncoderState>>,
    ffmpeg_path: PathBuf,
}

impl BackgroundEncoder {
    /// Create a new background encoder
    ///
    /// This spawns a background thread with its own Tokio runtime to process
    /// encoding tasks. The handle can be used to send commands, and the event
    /// receiver can be polled for progress updates.
    ///
    /// Returns (encoder, handle, event_receiver, output_manager) where:
    /// - event_receiver uses std::sync::mpsc so it can be polled from any thread without async
    /// - output_manager is the shared output manager used by the encoder (use this for ISO staging!)
    pub fn new() -> Result<(Self, BackgroundEncoderHandle, std_mpsc::Receiver<EncoderEvent>, OutputManager), String> {
        let output_manager = OutputManager::new()?;
        let ffmpeg_path = verify_ffmpeg()?;
        let state = Arc::new(Mutex::new(BackgroundEncoderState::new()));

        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = std_mpsc::channel();

        let encoder = Self {
            output_manager: output_manager.clone(),
            state: state.clone(),
            ffmpeg_path,
        };

        let handle = BackgroundEncoderHandle {
            command_tx,
            state: state.clone(),
        };

        // Start the encoder in a background thread with its own Tokio runtime
        // (GPUI doesn't provide a Tokio runtime, so we create one)
        let state_clone = state.clone();
        let output_manager_clone = output_manager.clone();
        let ffmpeg_path_clone = encoder.ffmpeg_path.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime for background encoder");

            rt.block_on(async move {
                run_encoder_loop(
                    state_clone,
                    output_manager_clone,
                    ffmpeg_path_clone,
                    command_rx,
                    event_tx,
                )
                .await;
            });
        });

        Ok((encoder, handle, event_rx, output_manager))
    }
}

/// Main encoder loop - processes commands and runs conversions
async fn run_encoder_loop(
    state: Arc<Mutex<BackgroundEncoderState>>,
    output_manager: OutputManager,
    ffmpeg_path: PathBuf,
    mut command_rx: mpsc::UnboundedReceiver<EncoderCommand>,
    event_tx: std_mpsc::Sender<EncoderEvent>,
) {
    state.lock().unwrap().is_running = true;
    println!("Background encoder started");

    loop {
        // Process any pending commands first
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
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
                    if let Some((active_id, cancel_token, _)) = &s.active {
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
                EncoderCommand::ClearAll => {
                    println!("Clearing all encoder state for new profile");
                    let mut s = state.lock().unwrap();
                    // Cancel any active encoding
                    if let Some((_, cancel_token, _)) = &s.active {
                        cancel_token.store(true, Ordering::SeqCst);
                    }
                    // Clear all state
                    s.queue.clear();
                    s.active = None;
                    s.completed.clear();
                    s.pending_bitrate_requeue = None;
                    drop(s);
                    // Clean up the session directory (delete all converted files)
                    if let Err(e) = output_manager.cleanup() {
                        eprintln!("Failed to clean up session: {}", e);
                    }
                    println!("Encoder state cleared");
                }
                EncoderCommand::RecalculateBitrate { target_bitrate } => {
                    // Collect re-encoding info with the lock held
                    let (old_bitrate, reencode_needed, folders_to_requeue) = {
                        let mut s = state.lock().unwrap();
                        let old_bitrate = s.lossless_bitrate;
                        s.lossless_bitrate = target_bitrate;

                        if old_bitrate == target_bitrate {
                            continue; // Skip all the work below
                        }

                        println!(
                            "Bitrate changed: {} -> {} kbps",
                            old_bitrate, target_bitrate
                        );

                        // Cancel in-progress lossless encoding
                        if let Some((active_id, cancel_token, has_lossless)) = &s.active {
                            if *has_lossless {
                                println!(
                                    "Cancelling in-progress lossless encoding for {:?}",
                                    active_id
                                );
                                cancel_token.store(true, Ordering::SeqCst);
                                // Mark for re-queue when cancellation is detected
                                s.pending_bitrate_requeue = Some(active_id.clone());
                            }
                        }

                        // Find completed folders that need re-encoding
                        let mut reencode_needed = Vec::new();
                        let mut folders_to_requeue = Vec::new();

                        for (id, (status, folder)) in &s.completed {
                            if let FolderConversionStatus::Converted {
                                lossless_bitrate: Some(br),
                                ..
                            } = status
                            {
                                if *br != target_bitrate {
                                    reencode_needed.push(id.clone());
                                    folders_to_requeue.push((id.clone(), folder.clone()));
                                }
                            }
                        }

                        // Remove from completed - they'll be re-queued
                        for id in &reencode_needed {
                            s.completed.remove(id);
                        }

                        (old_bitrate, reencode_needed, folders_to_requeue)
                    };

                    // Delete old output directories (outside lock)
                    for id in &reencode_needed {
                        let _ = output_manager.delete_folder_output(id);
                    }

                    // Re-queue folders for re-encoding
                    if !folders_to_requeue.is_empty() {
                        let mut s = state.lock().unwrap();
                        for (id, folder) in folders_to_requeue {
                            println!("Re-queuing folder for re-encoding: {}", folder.path.display());
                            s.queue.push_back((id, folder));
                        }
                    }

                    // Notify UI about re-encoding
                    if !reencode_needed.is_empty() {
                        println!(
                            "Folders queued for re-encoding: {}",
                            reencode_needed.len()
                        );
                        let _ = event_tx.send(EncoderEvent::BitrateRecalculated {
                            new_bitrate: target_bitrate,
                            reencode_needed,
                        });
                    }

                    let _ = old_bitrate; // suppress unused warning
                }
            }
        }

        // Check if there's work to do
        // Priority: process lossy-only folders first, then folders with lossless files
        // This ensures bitrate is stable before encoding lossless files
        let next_folder = {
            let mut s = state.lock().unwrap();
            if s.active.is_none() {
                // Look for a lossy-only folder first (no lossless files)
                let lossy_only_idx = s.queue.iter()
                    .position(|(_, folder)| !folder.has_lossless_files());

                if let Some(idx) = lossy_only_idx {
                    // Found a lossy-only folder - remove it from queue
                    s.queue.remove(idx)
                } else {
                    // No lossy-only folders, take from front (has lossless)
                    s.queue.pop_front()
                }
            } else {
                None
            }
        };

        if let Some((id, folder)) = next_folder {
            // Start encoding this folder
            let cancel_token = Arc::new(AtomicBool::new(false));
            let folder_has_lossless = folder.has_lossless_files();
            // Clone folder for storing in completed map (needed for re-encoding later)
            let folder_for_completed = folder.clone();
            {
                let mut s = state.lock().unwrap();
                s.active = Some((id.clone(), cancel_token.clone(), folder_has_lossless));
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
                    // Was cancelled - check if this was due to bitrate change
                    let should_requeue = s.pending_bitrate_requeue.as_ref() == Some(&id);
                    if should_requeue {
                        s.pending_bitrate_requeue = None;
                        // Re-queue for re-encoding at new bitrate
                        println!(
                            "Re-queuing cancelled lossless folder for re-encoding: {}",
                            folder_for_completed.path.display()
                        );
                        s.queue.push_back((id.clone(), folder_for_completed.clone()));
                        // Delete partial output
                        drop(s); // Release lock before file operations
                        let _ = output_manager.delete_folder_output(&id);
                    } else {
                        // Regular cancellation (folder removed by user)
                        let _ = event_tx.send(EncoderEvent::FolderCancelled(id.clone()));
                    }
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

                    s.completed.insert(id.clone(), (status, folder_for_completed));

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_is_pending() {
        let mut state = BackgroundEncoderState::new();
        let id1 = FolderId("folder1".to_string());
        let id2 = FolderId("folder2".to_string());
        let id3 = FolderId("folder3".to_string());

        assert!(!state.is_pending(&id1));

        // Add to queue
        state.queue.push_back((id1.clone(), MusicFolder::new_for_test_with_id("test")));
        assert!(state.is_pending(&id1));
        assert!(!state.is_pending(&id2));

        // Set active
        state.active = Some((id2.clone(), Arc::new(AtomicBool::new(false)), false));
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
            (
                FolderConversionStatus::Converted {
                    output_dir: PathBuf::from("/tmp/test"),
                    lossless_bitrate: Some(320),
                    output_size: 1000,
                    completed_at: 0,
                },
                MusicFolder::new_for_test_with_id("completed_folder"),
            ),
        );

        assert!(state.is_completed(&id));
    }
}
