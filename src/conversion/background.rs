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

/// Result of a folder conversion (sent back from spawned task)
struct FolderConversionResult {
    id: FolderId,
    folder: MusicFolder,
    output_dir: PathBuf,
    files_total: usize,
    completed: usize,
    was_cancelled: bool,
    has_lossless: bool,
    lossless_bitrate: u32,
}

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
    /// Import batch started - delay encoding until complete
    ImportStarted,
    /// Import batch completed - resume encoding
    ImportComplete,
    /// Update embed album art setting
    SetEmbedAlbumArt { embed: bool },
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
    /// Currently active folders: folder_id -> (cancel_token, has_lossless_files)
    pub active: HashMap<FolderId, (Arc<AtomicBool>, bool)>,
    /// Progress of active folders: folder_id -> (files_completed, files_total)
    pub active_progress: HashMap<FolderId, (usize, usize)>,
    /// Completed folders with their status and original folder data (for re-encoding)
    pub completed: HashMap<FolderId, (FolderConversionStatus, MusicFolder)>,
    /// Current lossless bitrate target
    pub lossless_bitrate: u32,
    /// Whether the encoder is running
    pub is_running: bool,
    /// Folder IDs that were cancelled due to bitrate change (needs re-queuing)
    pub pending_bitrate_requeue: Vec<FolderId>,
    /// Number of import batches in progress (don't start encoding while > 0)
    pub imports_pending: usize,
    /// Global pause flag - workers check this between files and wait if true
    pub paused: Arc<AtomicBool>,
    /// Whether to embed album art in output MP3s
    pub embed_album_art: bool,
}

impl BackgroundEncoderState {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            active: HashMap::new(),
            active_progress: HashMap::new(),
            completed: HashMap::new(),
            lossless_bitrate: 320, // Default to max quality
            is_running: false,
            pending_bitrate_requeue: Vec::new(),
            imports_pending: 0,
            paused: Arc::new(AtomicBool::new(false)),
            embed_album_art: false, // Default to stripping art for CD burning
        }
    }

    /// Check if a folder is queued or active
    pub fn is_pending(&self, id: &FolderId) -> bool {
        self.queue.iter().any(|(fid, _)| fid == id) || self.active.contains_key(id)
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

impl gpui::Global for BackgroundEncoderHandle {}

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

    /// Notify that an import batch has started (delays encoding)
    pub fn import_started(&self) {
        let _ = self.command_tx.send(EncoderCommand::ImportStarted);
    }

    /// Notify that an import batch has completed (resumes encoding)
    pub fn import_complete(&self) {
        let _ = self.command_tx.send(EncoderCommand::ImportComplete);
    }

    /// Update embed album art setting
    pub fn set_embed_album_art(&self, embed: bool) {
        let _ = self.command_tx.send(EncoderCommand::SetEmbedAlbumArt { embed });
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

    // Channel for receiving completion results from spawned folder tasks
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<FolderConversionResult>();

    loop {
        // Use select to handle both commands and folder completions
        tokio::select! {
            // Handle incoming commands
            Some(cmd) = command_rx.recv() => {
                handle_encoder_command(cmd, &state, &output_manager, &event_tx).await;
            }

            // Handle folder completion results
            Some(result) = completion_rx.recv() => {
                handle_folder_completion(result, &state, &output_manager, &event_tx).await;
            }

            // If no messages, try to start new folder conversions
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => {
                // Try to start new folders if we have capacity
                try_start_folders(
                    &state,
                    &output_manager,
                    &ffmpeg_path,
                    &event_tx,
                    &completion_tx,
                ).await;
            }
        }

        // Also try to start folders after handling any message
        try_start_folders(
            &state,
            &output_manager,
            &ffmpeg_path,
            &event_tx,
            &completion_tx,
        ).await;
    }
}

/// Handle an encoder command
async fn handle_encoder_command(
    cmd: EncoderCommand,
    state: &Arc<Mutex<BackgroundEncoderState>>,
    output_manager: &OutputManager,
    event_tx: &std_mpsc::Sender<EncoderEvent>,
) {
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
            if let Some((cancel_token, _)) = s.active.get(&id) {
                cancel_token.store(true, Ordering::SeqCst);
            }
            s.active.remove(&id);
            s.active_progress.remove(&id);
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
            for (cancel_token, _) in s.active.values() {
                cancel_token.store(true, Ordering::SeqCst);
            }
            // Clear all state
            s.queue.clear();
            s.active.clear();
            s.active_progress.clear();
            s.completed.clear();
            s.pending_bitrate_requeue.clear();
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
                    return; // Skip all the work below
                }

                println!(
                    "Bitrate changed: {} -> {} kbps",
                    old_bitrate, target_bitrate
                );

                // Cancel in-progress lossless encoding for all active folders
                let mut to_requeue = Vec::new();
                for (active_id, (cancel_token, has_lossless)) in &s.active {
                    if *has_lossless {
                        println!(
                            "Cancelling in-progress lossless encoding for {:?}",
                            active_id
                        );
                        cancel_token.store(true, Ordering::SeqCst);
                        // Mark for re-queue when cancellation is detected
                        to_requeue.push(active_id.clone());
                    }
                }
                s.pending_bitrate_requeue.extend(to_requeue);

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
        EncoderCommand::ImportStarted => {
            let mut s = state.lock().unwrap();
            s.imports_pending = s.imports_pending.saturating_add(1);
            // Pause encoding while importing
            s.paused.store(true, Ordering::SeqCst);
            println!("Import started, pending: {} (encoding paused)", s.imports_pending);
        }
        EncoderCommand::ImportComplete => {
            let mut s = state.lock().unwrap();
            s.imports_pending = s.imports_pending.saturating_sub(1);
            // Resume encoding when all imports complete
            if s.imports_pending == 0 {
                s.paused.store(false, Ordering::SeqCst);
                println!("Import complete, pending: 0 (encoding resumed)");
            } else {
                println!("Import complete, pending: {} (still paused)", s.imports_pending);
            }
        }
        EncoderCommand::SetEmbedAlbumArt { embed } => {
            let mut s = state.lock().unwrap();
            s.embed_album_art = embed;
            println!("Embed album art: {}", embed);
        }
    }
}

/// Handle a folder conversion completion
async fn handle_folder_completion(
    result: FolderConversionResult,
    state: &Arc<Mutex<BackgroundEncoderState>>,
    output_manager: &OutputManager,
    event_tx: &std_mpsc::Sender<EncoderEvent>,
) {
    let FolderConversionResult {
        id,
        folder,
        output_dir,
        files_total,
        completed,
        was_cancelled,
        has_lossless,
        lossless_bitrate,
    } = result;

    let mut s = state.lock().unwrap();

    // Check if cancel token was set (might have been cancelled after task started)
    let token_cancelled = s.active.get(&id)
        .map(|(token, _)| token.load(Ordering::SeqCst))
        .unwrap_or(false);

    s.active.remove(&id);
    s.active_progress.remove(&id);

    if was_cancelled || token_cancelled {
        // Was cancelled - check if this was due to bitrate change
        let should_requeue = s.pending_bitrate_requeue.contains(&id);
        if should_requeue {
            s.pending_bitrate_requeue.retain(|x| x != &id);
            // Re-queue for re-encoding at new bitrate
            println!(
                "Re-queuing cancelled lossless folder for re-encoding: {}",
                folder.path.display()
            );
            s.queue.push_back((id.clone(), folder));
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

        s.completed.insert(id.clone(), (status, folder));

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

/// Try to start new folder conversions if we have capacity
async fn try_start_folders(
    state: &Arc<Mutex<BackgroundEncoderState>>,
    output_manager: &OutputManager,
    ffmpeg_path: &PathBuf,
    event_tx: &std_mpsc::Sender<EncoderEvent>,
    completion_tx: &mpsc::UnboundedSender<FolderConversionResult>,
) {
    const MAX_CONCURRENT_FOLDERS: usize = 3;

    // Keep starting folders until we hit capacity or run out of work
    loop {
        let next_folder = {
            let mut s = state.lock().unwrap();

            // Don't start encoding while imports are in progress
            if s.imports_pending > 0 {
                return;
            }

            if s.active.len() >= MAX_CONCURRENT_FOLDERS {
                return; // At capacity
            }

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
        };

        let Some((id, folder)) = next_folder else {
            return; // No work to do
        };

        // Start encoding this folder
        let cancel_token = Arc::new(AtomicBool::new(false));
        let folder_has_lossless = folder.has_lossless_files();
        let folder_for_task = folder.clone();
        {
            let mut s = state.lock().unwrap();
            s.active.insert(id.clone(), (cancel_token.clone(), folder_has_lossless));
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
                state.lock().unwrap().active.remove(&id);
                continue; // Try next folder
            }
        };

        // Get current settings from state
        let (lossless_bitrate, embed_album_art) = {
            let state_guard = state.lock().unwrap();
            (state_guard.lossless_bitrate, state_guard.embed_album_art)
        };

        // Build conversion jobs
        let mut jobs = Vec::new();
        let mut has_lossless = false;

        for file in &folder.audio_files {
            let strategy = determine_encoding_strategy(
                &file.codec,
                file.bitrate,
                lossless_bitrate,
                file.is_lossy,
                false, // no_lossy_mode
                embed_album_art,
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

            // Only pass album art path if embed_album_art is enabled and we have art
            let album_art_path = if embed_album_art {
                folder.album_art.as_ref().map(|s| PathBuf::from(s))
            } else {
                None
            };

            jobs.push(ConversionJob {
                input_path: file.path.clone(),
                output_path,
                strategy,
                album_art_path,
            });
        }

        // Sort jobs: Copy first, then lossy, then lossless
        jobs.sort_by_key(|j| match &j.strategy {
            EncodingStrategy::Copy => 0,
            EncodingStrategy::CopyWithoutArt => 1,
            EncodingStrategy::ConvertAtSourceBitrate(_) => 2,
            EncodingStrategy::ConvertAtTargetBitrate(_) => 3,
        });

        // Clone what we need for the spawned task
        let ffmpeg_path = ffmpeg_path.clone();
        let event_tx = event_tx.clone();
        let completion_tx = completion_tx.clone();
        let id_for_task = id.clone();
        let output_dir_for_task = output_dir.clone();
        let state_for_progress = state.clone();
        let pause_token = state.lock().unwrap().paused.clone();

        // Spawn the conversion task
        tokio::spawn(async move {
            let progress = Arc::new(ConversionProgress::new(jobs.len()));
            let progress_for_callback = progress.clone();
            let id_for_callback = id_for_task.clone();
            let event_tx_for_callback = event_tx.clone();
            let state_for_callback = state_for_progress.clone();

            let (completed, _failed, was_cancelled) = convert_files_parallel_with_callback(
                ffmpeg_path,
                jobs,
                progress.clone(),
                cancel_token.clone(),
                pause_token,
                move || {
                    let completed = progress_for_callback.completed_count();
                    let total = progress_for_callback.total;

                    // Update state for UI
                    if let Ok(mut s) = state_for_callback.lock() {
                        s.active_progress.insert(id_for_callback.clone(), (completed, total));
                    }

                    let _ = event_tx_for_callback.send(EncoderEvent::FolderProgress {
                        id: id_for_callback.clone(),
                        files_completed: completed,
                        files_total: total,
                    });
                },
            )
            .await;

            // Send completion result back to main loop
            let _ = completion_tx.send(FolderConversionResult {
                id: id_for_task,
                folder: folder_for_task,
                output_dir: output_dir_for_task,
                files_total,
                completed,
                was_cancelled: was_cancelled || cancel_token.load(Ordering::SeqCst),
                has_lossless,
                lossless_bitrate,
            });
        });

        println!("Spawned encoding task for folder: {}", folder.path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_background_encoder_state_new() {
        let state = BackgroundEncoderState::new();
        assert_eq!(state.queue.len(), 0);
        assert!(state.active.is_empty());
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
        state.active.insert(id2.clone(), (Arc::new(AtomicBool::new(false)), false));
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
