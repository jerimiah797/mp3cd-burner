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

/// Encoding phase for two-pass optimization
///
/// Two-pass encoding maximizes CD utilization:
/// - Pass 1: Encode lossy files at source bitrate (size is predictable)
/// - Measure actual sizes after pass 1
/// - Pass 2: Encode lossless files at optimized bitrate based on remaining space
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingPhase {
    /// Initial state - no encoding in progress
    Idle,
    /// Pass 1: Encoding lossy files, lossless files wait
    LossyPass,
    /// Pass 2: All lossy complete, encoding lossless at optimized bitrate
    LosslessPass,
}

impl Default for EncodingPhase {
    fn default() -> Self {
        Self::Idle
    }
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
    /// Encoding phase changed (pass 1 → pass 2)
    PhaseTransition {
        phase: EncodingPhase,
        measured_lossy_size: u64,
        optimal_bitrate: u32,
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

    // === Two-pass encoding state ===

    /// Current encoding phase (Idle, LossyPass, or LosslessPass)
    pub encoding_phase: EncodingPhase,
    /// Folders with lossless files waiting for pass 2
    /// These are queued for pass 1 (lossy only) but held here for pass 2 (lossless)
    pub lossless_pending: Vec<(FolderId, MusicFolder)>,
    /// Actual encoded size of lossy files (measured after pass 1 completes)
    pub measured_lossy_size: u64,
    /// Folders that completed pass 1 (used to know which folders to skip lossy files for in pass 2)
    pub pass1_completed: std::collections::HashSet<FolderId>,
    /// Size of lossy folders added after pass 1 (needs to be included in bitrate recalculation)
    pub late_lossy_size: u64,
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
            // Two-pass encoding state
            encoding_phase: EncodingPhase::Idle,
            lossless_pending: Vec::new(),
            measured_lossy_size: 0,
            pass1_completed: std::collections::HashSet::new(),
            late_lossy_size: 0,
        }
    }

    /// Check if a folder is queued, active, or waiting for pass 2
    pub fn is_pending(&self, id: &FolderId) -> bool {
        self.queue.iter().any(|(fid, _)| fid == id)
            || self.active.contains_key(id)
            || self.lossless_pending.iter().any(|(fid, _)| fid == id)
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
            let reset_result = {
                let mut s = state.lock().unwrap();

                // Don't add if already queued/active/completed
                if s.is_pending(&id) || s.is_completed(&id) {
                    None // Already exists, nothing to do
                } else {
                    // If we're in LosslessPass, adding a folder changes the space budget
                    // Reset to LossyPass to recalculate everything
                    let reset_result = if s.encoding_phase == EncodingPhase::LosslessPass {
                        Some(reset_to_lossy_pass(&mut s))
                    } else {
                        None
                    };

                    if folder.has_lossless_files() {
                        // Two-pass encoding: folder has lossless files
                        // - Add to lossless_pending (for pass 2 lossless encoding)
                        // - Also add to queue (for pass 1 lossy-only encoding)
                        println!(
                            "Queued folder for two-pass encoding: {} (has lossless)",
                            folder.path.display()
                        );
                        s.lossless_pending.push((id.clone(), folder.clone()));
                        s.queue.push_back((id, folder));
                        // If we were idle, switch to lossy pass
                        if s.encoding_phase == EncodingPhase::Idle {
                            s.encoding_phase = EncodingPhase::LossyPass;
                        }
                    } else {
                        // Pure lossy folder - single pass encoding
                        println!(
                            "Queued folder for encoding: {} (lossy only)",
                            folder.path.display()
                        );
                        s.queue.push_back((id, folder));
                    }
                    reset_result
                }
            };
            // Handle reset: delete output files and notify UI
            if let Some(result) = reset_result {
                for id in result.ids_to_delete {
                    let _ = output_manager.delete_folder_output(&id);
                }
                // Notify UI that these folders need re-encoding
                if !result.ids_requeued.is_empty() {
                    let _ = event_tx.send(EncoderEvent::BitrateRecalculated {
                        new_bitrate: 0, // Will be recalculated during phase transition
                        reencode_needed: result.ids_requeued,
                    });
                }
            }
        }
        EncoderCommand::RemoveFolder(id) => {
            let reset_result = {
                let mut s = state.lock().unwrap();

                println!(
                    "RemoveFolder: {:?}, current phase: {:?}, queue: {}, active: {}, completed: {}, lossless_pending: {}",
                    id, s.encoding_phase, s.queue.len(), s.active.len(), s.completed.len(), s.lossless_pending.len()
                );

                // Remove from queue if present
                s.queue.retain(|(fid, _)| fid != &id);
                // Remove from lossless_pending if present (two-pass state)
                s.lossless_pending.retain(|(fid, _)| fid != &id);
                // Cancel if active
                if let Some((cancel_token, _)) = s.active.get(&id) {
                    cancel_token.store(true, Ordering::SeqCst);
                }
                s.active.remove(&id);
                s.active_progress.remove(&id);
                // Remove from completed
                let was_completed = s.completed.remove(&id).is_some();
                println!("  -> was_completed: {}", was_completed);

                // If we're in LosslessPass, removing a folder changes the space budget
                // Reset to LossyPass to recalculate lossless bitrate
                if s.encoding_phase == EncodingPhase::LosslessPass {
                    println!("  -> triggering reset to LossyPass");
                    Some(reset_to_lossy_pass(&mut s))
                } else {
                    println!("  -> NOT in LosslessPass, skipping reset");
                    None
                }
            };

            // Clean up output directory (outside lock)
            let _ = output_manager.delete_folder_output(&id);

            // Handle reset: delete output files and notify UI
            if let Some(result) = reset_result {
                for folder_id in result.ids_to_delete {
                    let _ = output_manager.delete_folder_output(&folder_id);
                }
                // Notify UI that these folders need re-encoding
                if !result.ids_requeued.is_empty() {
                    let _ = event_tx.send(EncoderEvent::BitrateRecalculated {
                        new_bitrate: 0, // Will be recalculated during phase transition
                        reencode_needed: result.ids_requeued,
                    });
                }
            }

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
            // Reset two-pass encoding state
            s.lossless_pending.clear();
            s.encoding_phase = EncodingPhase::Idle;
            s.measured_lossy_size = 0;
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

    // Track if we need to delete partial output (for requeue case)
    let mut need_delete_output = false;

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
            need_delete_output = true;
        } else {
            // Regular cancellation (folder removed by user)
            let _ = event_tx.send(EncoderEvent::FolderCancelled(id.clone()));
        }
    } else if completed == files_total || files_total == 0 {
        // Success! (files_total == 0 means empty pass, e.g., pure lossless in LossyPass)
        let output_size = output_manager
            .get_folder_output_size(&id)
            .unwrap_or(0);

        // Check if this folder is still waiting for pass 2
        let still_in_lossless_pending = s.lossless_pending.iter().any(|(fid, _)| fid == &id);

        // A folder is "pass 1 complete" (not fully done) if:
        // 1. It's still in lossless_pending (has lossless files to encode in pass 2), AND
        // 2. Either we're in LossyPass, OR this was an empty pass (files_total == 0)
        //    The empty pass check handles race conditions where completion is processed
        //    after phase transition due to async timing
        let is_pass1_complete = still_in_lossless_pending
            && (s.encoding_phase == EncodingPhase::LossyPass || files_total == 0);

        if is_pass1_complete {
            // Pass 1 complete for this folder, but it has lossless files pending for pass 2
            // Don't add to completed yet - it will be fully completed in pass 2
            println!(
                "Pass 1 complete for folder {} (lossy done, lossless pending)",
                folder.path.display()
            );
            // Update measured_lossy_size with this folder's output (only in LossyPass)
            if s.encoding_phase == EncodingPhase::LossyPass {
                s.measured_lossy_size += output_size;
            }
            // Mark this folder as having completed pass 1 (so we skip lossy files in pass 2)
            s.pass1_completed.insert(id.clone());
        } else {
            // Fully complete (either pure lossy folder, or pass 2 complete)
            let status = FolderConversionStatus::Converted {
                output_dir: output_dir.clone(),
                lossless_bitrate: if has_lossless { Some(lossless_bitrate) } else { None },
                output_size,
                completed_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            s.completed.insert(id.clone(), (status, folder.clone()));

            // If we just finished pass 2 for this folder, remove it from lossless_pending
            if s.encoding_phase == EncodingPhase::LosslessPass {
                s.lossless_pending.retain(|(fid, _)| fid != &id);
            }

            let _ = event_tx.send(EncoderEvent::FolderCompleted {
                id: id.clone(),
                output_dir,
                output_size,
                lossless_bitrate: if has_lossless { Some(lossless_bitrate) } else { None },
            });
        }
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

    // Release lock before file operations
    drop(s);

    // Delete partial output if needed (for requeue case)
    if need_delete_output {
        let _ = output_manager.delete_folder_output(&id);
    }

    // Check if we should transition to pass 2
    check_phase_transition(state, output_manager, event_tx).await;
}

/// Check if pass 1 is complete and transition to pass 2 if needed
async fn check_phase_transition(
    state: &Arc<Mutex<BackgroundEncoderState>>,
    _output_manager: &OutputManager,
    event_tx: &std_mpsc::Sender<EncoderEvent>,
) {
    let should_transition = {
        let s = state.lock().unwrap();

        // Only transition from LossyPass to LosslessPass
        if s.encoding_phase != EncodingPhase::LossyPass {
            return;
        }

        // Check if pass 1 is complete:
        // - Queue is empty
        // - No active folders
        // - There are folders waiting in lossless_pending
        s.queue.is_empty() && s.active.is_empty() && !s.lossless_pending.is_empty()
    };

    if !should_transition {
        return;
    }

    // Transition to pass 2
    transition_to_pass2(state, event_tx).await;
}

/// Transition from pass 1 (lossy) to pass 2 (lossless) with bitrate recalculation
async fn transition_to_pass2(
    state: &Arc<Mutex<BackgroundEncoderState>>,
    event_tx: &std_mpsc::Sender<EncoderEvent>,
) {
    println!("=== PHASE TRANSITION: Pass 1 complete, transitioning to Pass 2 ===");

    // Calculate optimal lossless bitrate based on remaining CD space
    let (measured_size, lossless_duration, folders_to_queue, optimal_bitrate) = {
        let s = state.lock().unwrap();

        // Sum up actual output sizes of completed folders + measured_lossy_size
        // (measured_lossy_size contains sizes from pass 1 for folders still in lossless_pending)
        let completed_size: u64 = s.completed.values()
            .map(|(status, _)| {
                if let FolderConversionStatus::Converted { output_size, .. } = status {
                    *output_size
                } else {
                    0
                }
            })
            .sum();

        let total_lossy_size = completed_size + s.measured_lossy_size;

        // Calculate total duration of pending lossless files
        let lossless_duration: f64 = s.lossless_pending.iter()
            .flat_map(|(_, folder)| &folder.audio_files)
            .filter(|f| !f.is_lossy)
            .map(|f| f.duration)
            .sum();

        // Calculate optimal bitrate for lossless files
        const CD_CAPACITY: u64 = 700 * 1000 * 1000;
        const SAFETY_MARGIN: f64 = 0.98; // 2% safety margin

        let remaining_space = ((CD_CAPACITY as f64 * SAFETY_MARGIN) as u64)
            .saturating_sub(total_lossy_size);

        let optimal_bitrate = if lossless_duration > 0.0 {
            let bitrate = ((remaining_space * 8) as f64 / lossless_duration / 1000.0) as u32;
            bitrate.clamp(64, 320)
        } else {
            320
        };

        let folders = s.lossless_pending.clone();

        (total_lossy_size, lossless_duration, folders, optimal_bitrate)
    };

    println!(
        "Pass 2 calculation: Measured lossy size = {} MB, Lossless duration = {:.0}s, Remaining space = {} MB, Optimal bitrate = {} kbps",
        measured_size / 1_000_000,
        lossless_duration,
        ((700 * 1000 * 1000_u64).saturating_sub(measured_size)) / 1_000_000,
        optimal_bitrate
    );

    // Update state and queue lossless folders for pass 2
    {
        let mut s = state.lock().unwrap();
        s.encoding_phase = EncodingPhase::LosslessPass;
        s.lossless_bitrate = optimal_bitrate;

        // Queue folders from lossless_pending for pass 2
        for (id, folder) in &folders_to_queue {
            println!("Queuing folder for pass 2: {}", folder.path.display());
            s.queue.push_back((id.clone(), folder.clone()));
        }
        // Don't clear lossless_pending yet - we use it to track which folders are in pass 2
        // It will be cleared when the folder is fully completed
    }

    // Notify UI about phase transition
    let _ = event_tx.send(EncoderEvent::PhaseTransition {
        phase: EncodingPhase::LosslessPass,
        measured_lossy_size: measured_size,
        optimal_bitrate,
    });

    println!("=== Pass 2 started: encoding lossless files at {} kbps ===", optimal_bitrate);
}

/// Result of resetting from LosslessPass to LossyPass
struct ResetResult {
    /// Folder IDs whose output should be deleted
    ids_to_delete: Vec<FolderId>,
    /// Folder IDs that were re-queued for encoding (for UI notification)
    ids_requeued: Vec<FolderId>,
}

/// Reset from LosslessPass back to LossyPass when folders change
///
/// This is called when a folder is added or removed during LosslessPass.
/// It cancels active lossless encoding, moves completed lossless folders back
/// to lossless_pending for re-encoding at a new bitrate.
fn reset_to_lossy_pass(s: &mut BackgroundEncoderState) -> ResetResult {
    println!("=== RESET: Folder change during LosslessPass - resetting to LossyPass ===");

    // Cancel active lossless encoding and collect those folder IDs
    let mut active_lossless_ids = Vec::new();
    for (id, (cancel_token, has_lossless)) in &s.active {
        if *has_lossless {
            println!("Cancelling active lossless encoding for {:?}", id);
            cancel_token.store(true, Ordering::SeqCst);
            active_lossless_ids.push(id.clone());
        }
    }

    // Find completed lossless folders that need re-encoding
    let lossless_to_requeue: Vec<_> = s
        .completed
        .iter()
        .filter(|(_, (status, _))| {
            matches!(
                status,
                FolderConversionStatus::Converted {
                    lossless_bitrate: Some(_),
                    ..
                }
            )
        })
        .map(|(id, (_, folder))| (id.clone(), folder.clone()))
        .collect();

    let ids_to_delete: Vec<FolderId> = lossless_to_requeue
        .iter()
        .map(|(id, _)| id.clone())
        .collect();

    // Collect all IDs that need re-encoding (both completed and active lossless)
    let mut ids_requeued: Vec<FolderId> = ids_to_delete.clone();
    ids_requeued.extend(active_lossless_ids);

    // Move them back to lossless_pending and queue
    for (id, folder) in lossless_to_requeue {
        println!(
            "Re-queuing lossless folder for re-encoding: {}",
            folder.path.display()
        );
        s.completed.remove(&id);
        s.lossless_pending.push((id.clone(), folder.clone()));
        s.queue.push_back((id, folder));
    }

    // Reset phase and counters
    s.encoding_phase = EncodingPhase::LossyPass;
    s.measured_lossy_size = 0;
    s.late_lossy_size = 0;
    s.pass1_completed.clear();

    ResetResult {
        ids_to_delete,
        ids_requeued,
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
        let (lossless_bitrate, embed_album_art, encoding_phase, folder_did_pass1) = {
            let state_guard = state.lock().unwrap();
            let did_pass1 = state_guard.pass1_completed.contains(&id);
            (state_guard.lossless_bitrate, state_guard.embed_album_art, state_guard.encoding_phase, did_pass1)
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

            let is_lossless_file = matches!(strategy, EncodingStrategy::ConvertAtTargetBitrate(_));

            if is_lossless_file {
                has_lossless = true;
            }

            // Two-pass filtering:
            // - In LossyPass: skip lossless files (they'll be encoded in pass 2)
            // - In LosslessPass: skip lossy files ONLY if this folder completed pass 1
            //   (new folders added during LosslessPass should encode all their files)
            match encoding_phase {
                EncodingPhase::LossyPass if is_lossless_file => {
                    // Skip lossless files in pass 1
                    continue;
                }
                EncodingPhase::LosslessPass if !is_lossless_file && folder_did_pass1 => {
                    // Skip lossy files in pass 2 (already encoded in pass 1)
                    continue;
                }
                _ => {}
            }

            let output_name = file
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let output_path = output_dir.join(format!("{}.mp3", output_name));

            // Skip files that are already encoded (for efficient "resume" after reset)
            // This allows lossy files to be reused when we reset from LosslessPass to LossyPass
            if output_path.exists() {
                println!(
                    "Skipping already-encoded file: {}",
                    output_path.display()
                );
                continue;
            }

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

        // Handle empty jobs case (e.g., pure lossless folder in LossyPass)
        if jobs.is_empty() {
            // No files to encode in this pass - mark as immediately "complete" for this pass
            // The folder will be processed in the next pass if it's in lossless_pending
            println!(
                "No files to encode for {:?} in {:?} phase - skipping",
                id, encoding_phase
            );
            state.lock().unwrap().active.remove(&id);

            // Send a completion result to trigger pass completion check
            let _ = completion_tx.send(FolderConversionResult {
                id: id.clone(),
                folder: folder_for_task,
                output_dir: output_dir.clone(),
                files_total: 0,
                completed: 0,
                was_cancelled: false,
                has_lossless,
                lossless_bitrate,
            });

            continue; // Try next folder
        }

        // Update files_total to reflect actual jobs being processed in this pass
        let files_total = jobs.len();
        let _ = event_tx.send(EncoderEvent::FolderStarted {
            id: id.clone(),
            files_total,
        });

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
