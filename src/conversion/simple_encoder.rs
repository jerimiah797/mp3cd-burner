//! Simplified Background Encoder
//!
//! Core principle: Folder list is source of truth. Encoding is stateless and restartable.
//! When anything changes → restart fresh. Use file existence to skip done work.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::core::{AudioFileInfo, FolderId, MusicFolder};
use super::background::EncoderEvent;
use super::output_manager::OutputManager;

// Re-export EncodingPhase from background module
pub use super::background::EncodingPhase;

/// Shared encoder state (for UI to read)
pub struct SimpleEncoderState {
    /// Current phase
    pub phase: Mutex<EncodingPhase>,
    /// Current lossless bitrate
    pub lossless_bitrate: AtomicU32,
    /// Restart requested flag
    restart_requested: AtomicBool,
    /// Pause flag (for imports)
    paused: AtomicBool,
    /// Embed album art setting
    embed_album_art: AtomicBool,
    /// Currently encoding folder (if any)
    pub current_folder: Mutex<Option<FolderId>>,
    /// Progress within current folder
    pub current_progress: Mutex<(usize, usize)>, // (completed, total)
    /// Manual bitrate override (None = auto-calculate)
    pub manual_bitrate: Mutex<Option<u32>>,
}

impl SimpleEncoderState {
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(EncodingPhase::Idle),
            lossless_bitrate: AtomicU32::new(320),
            restart_requested: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            embed_album_art: AtomicBool::new(false),
            current_folder: Mutex::new(None),
            current_progress: Mutex::new((0, 0)),
            manual_bitrate: Mutex::new(None),
        }
    }

    pub fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::SeqCst);
    }

    pub fn is_restart_requested(&self) -> bool {
        self.restart_requested.load(Ordering::SeqCst)
    }

    pub fn clear_restart(&self) {
        self.restart_requested.store(false, Ordering::SeqCst);
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn get_phase(&self) -> EncodingPhase {
        *self.phase.lock().unwrap()
    }

    pub fn set_phase(&self, phase: EncodingPhase) {
        *self.phase.lock().unwrap() = phase;
    }
}

/// Handle for controlling the encoder from the UI
#[derive(Clone)]
pub struct SimpleEncoderHandle {
    state: Arc<SimpleEncoderState>,
    output_manager: Arc<OutputManager>,
    /// Shared folder list - FolderList updates this, encoder reads from it
    shared_folders: Arc<Mutex<Vec<MusicFolder>>>,
    /// Channel to send progress updates (unused by handle, but kept for API compatibility)
    #[allow(dead_code)]
    progress_tx: mpsc::Sender<EncoderEvent>,
}

// Implement Global for GPUI global access
impl gpui::Global for SimpleEncoderHandle {}

impl SimpleEncoderHandle {
    /// Signal that folders changed - restart encoding
    pub fn restart(&self) {
        println!("Encoder: restart requested");
        self.state.request_restart();
    }

    /// Pause encoding (for imports)
    pub fn pause(&self) {
        println!("Encoder: paused");
        self.state.set_paused(true);
    }

    /// Resume encoding after import
    pub fn resume(&self) {
        println!("Encoder: resumed");
        self.state.set_paused(false);
        // Also restart to pick up new folders
        self.state.request_restart();
    }

    /// Set manual bitrate override
    #[allow(dead_code)]
    pub fn set_manual_bitrate(&self, bitrate: Option<u32>) {
        *self.state.manual_bitrate.lock().unwrap() = bitrate;
        self.restart();
    }

    /// Set embed album art setting
    pub fn set_embed_album_art(&self, embed: bool) {
        self.state.embed_album_art.store(embed, Ordering::SeqCst);
        // No restart needed - will apply to new encodings
    }

    /// Clear all state (for New profile)
    pub fn clear(&self) {
        self.state.set_phase(EncodingPhase::Idle);
        self.state.clear_restart();
        *self.state.current_folder.lock().unwrap() = None;
        *self.state.current_progress.lock().unwrap() = (0, 0);
        *self.state.manual_bitrate.lock().unwrap() = None;
        // Also clean up output files
        let _ = self.output_manager.cleanup();
    }

    /// Get state for UI reading
    pub fn get_state(&self) -> Arc<SimpleEncoderState> {
        self.state.clone()
    }

    /// Get output manager
    #[allow(dead_code)]
    pub fn get_output_manager(&self) -> Arc<OutputManager> {
        self.output_manager.clone()
    }

    /// Update the shared folder list (called by FolderList when folders change)
    #[allow(dead_code)]
    pub fn update_folders(&self, folders: Vec<MusicFolder>) {
        *self.shared_folders.lock().unwrap() = folders;
    }

    /// Get the shared folder list Arc (for FolderList to store and update directly)
    pub fn get_shared_folders(&self) -> Arc<Mutex<Vec<MusicFolder>>> {
        self.shared_folders.clone()
    }

    // === Compatibility methods for BackgroundEncoderHandle API ===

    /// Add a folder (compatibility - updates shared folders and restarts)
    pub fn add_folder(&self, folder: MusicFolder) {
        let mut guard = self.shared_folders.lock().unwrap();
        if !guard.iter().any(|f| f.id == folder.id) {
            guard.push(folder);
        }
        drop(guard);
        self.restart();
    }

    /// Remove a folder (compatibility - updates shared folders and restarts)
    pub fn remove_folder(&self, id: &crate::core::FolderId) {
        let mut guard = self.shared_folders.lock().unwrap();
        guard.retain(|f| &f.id != id);
        drop(guard);
        self.restart();
    }

    /// Notify folders reordered (compatibility - just restarts)
    pub fn folders_reordered(&self) {
        self.restart();
    }

    /// Recalculate bitrate (compatibility - restarts to recalculate)
    pub fn recalculate_bitrate(&self, _target: u32) {
        // The simple encoder calculates bitrate fresh on each pass
        self.restart();
    }

    /// Clear all state (compatibility - alias for clear())
    pub fn clear_all(&self) {
        self.clear();
        // Also clear the shared folders
        self.shared_folders.lock().unwrap().clear();
    }

    /// Import started (compatibility - alias for pause())
    pub fn import_started(&self) {
        self.pause();
    }

    /// Import complete (compatibility - alias for resume())
    pub fn import_complete(&self) {
        self.resume();
    }

    /// Register completed folder (compatibility - adds to shared folders with status)
    pub fn register_completed(
        &self,
        folder: MusicFolder,
        _output_dir: PathBuf,
        _output_size: u64,
        _lossless_bitrate: Option<u32>,
        _completed_at: u64,
    ) {
        // For pre-converted folders, just add them to the list
        // The encoder will skip them because output files already exist
        let mut guard = self.shared_folders.lock().unwrap();
        if !guard.iter().any(|f| f.id == folder.id) {
            guard.push(folder);
        }
        drop(guard);
        // Don't restart - these are already complete
    }
}

/// Start the simple encoder
pub fn start_simple_encoder(
    output_manager: Arc<OutputManager>,
    ffmpeg_path: PathBuf,
) -> (SimpleEncoderHandle, mpsc::Receiver<EncoderEvent>) {
    let state = Arc::new(SimpleEncoderState::new());
    let shared_folders: Arc<Mutex<Vec<MusicFolder>>> = Arc::new(Mutex::new(Vec::new()));
    let (progress_tx, progress_rx) = mpsc::channel();

    let handle = SimpleEncoderHandle {
        state: state.clone(),
        output_manager: output_manager.clone(),
        shared_folders: shared_folders.clone(),
        progress_tx: progress_tx.clone(),
    };

    // Start the encoding thread
    let state_clone = state.clone();
    let output_manager_clone = output_manager.clone();
    let shared_folders_clone = shared_folders.clone();
    thread::spawn(move || {
        encoding_loop(
            state_clone,
            output_manager_clone,
            shared_folders_clone,
            progress_tx,
            ffmpeg_path,
        );
    });

    (handle, progress_rx)
}

/// Main encoding loop
fn encoding_loop(
    state: Arc<SimpleEncoderState>,
    output_manager: Arc<OutputManager>,
    shared_folders: Arc<Mutex<Vec<MusicFolder>>>,
    progress_tx: mpsc::Sender<EncoderEvent>,
    ffmpeg_path: PathBuf,
) {
    println!("Simple encoder loop started");

    loop {
        // Wait until we have work to do
        loop {
            if state.is_paused() {
                thread::sleep(Duration::from_millis(100));
                continue;
            }

            // Check if we have folders
            let folders = shared_folders.lock().unwrap().clone();

            if folders.is_empty() {
                state.set_phase(EncodingPhase::Idle);
                thread::sleep(Duration::from_millis(100));
                continue;
            }

            // We have work - break out to start encoding
            break;
        }

        // Clear restart flag before starting
        state.clear_restart();

        // Get current folders (snapshot at start of encoding pass)
        let folders: Vec<MusicFolder> = shared_folders.lock().unwrap().clone();

        if folders.is_empty() {
            continue;
        }

        println!("Starting encoding: {} folders", folders.len());

        // === PHASE 1: Lossy files ===
        state.set_phase(EncodingPhase::LossyPass);
        let _ = progress_tx.send(EncoderEvent::PhaseTransition {
            phase: EncodingPhase::LossyPass,
            measured_lossy_size: 0,
            optimal_bitrate: 320,
        });

        let mut restart_needed = false;
        for (_folder_idx, folder) in folders.iter().enumerate() {
            if state.is_restart_requested() {
                restart_needed = true;
                break;
            }

            let lossy_files: Vec<&AudioFileInfo> =
                folder.audio_files.iter().filter(|f| f.is_lossy).collect();

            if lossy_files.is_empty() {
                continue;
            }

            let _ = progress_tx.send(EncoderEvent::FolderStarted {
                id: folder.id.clone(),
                files_total: lossy_files.len(),
            });

            *state.current_folder.lock().unwrap() = Some(folder.id.clone());
            *state.current_progress.lock().unwrap() = (0, lossy_files.len());

            let output_dir = match output_manager.get_folder_output_dir(&folder.id) {
                Ok(dir) => dir,
                Err(e) => {
                    eprintln!("Failed to get output dir for {:?}: {}", folder.id, e);
                    continue;
                }
            };
            let embed_art = state.embed_album_art.load(Ordering::SeqCst);
            let album_art = if embed_art {
                folder.album_art.clone()
            } else {
                None
            };

            for (file_idx, file) in lossy_files.iter().enumerate() {
                if state.is_restart_requested() {
                    restart_needed = true;
                    break;
                }

                let output_path = get_output_path(&output_dir, &file.path);

                // Skip if already encoded
                if output_path.exists() {
                    *state.current_progress.lock().unwrap() = (file_idx + 1, lossy_files.len());
                    continue;
                }

                // Encode at source bitrate
                let bitrate = file.bitrate;
                if let Err(e) = transcode_file(
                    &ffmpeg_path,
                    &file.path,
                    &output_path,
                    bitrate,
                    album_art.as_ref().map(|s| Path::new(s.as_str())),
                ) {
                    eprintln!("Failed to encode {:?}: {}", file.path, e);
                }

                *state.current_progress.lock().unwrap() = (file_idx + 1, lossy_files.len());
                let _ = progress_tx.send(EncoderEvent::FolderProgress {
                    id: folder.id.clone(),
                    files_completed: file_idx + 1,
                    files_total: lossy_files.len(),
                });
            }

            if restart_needed {
                break;
            }

            let output_size = output_manager.get_folder_output_size(&folder.id).unwrap_or(0);
            let _ = progress_tx.send(EncoderEvent::FolderCompleted {
                id: folder.id.clone(),
                output_dir: output_dir.clone(),
                output_size,
                lossless_bitrate: None, // Lossy pass, no lossless bitrate
            });
        }

        if restart_needed {
            println!("Restart requested during lossy pass");
            continue;
        }

        // === MEASURE & CALCULATE BITRATE ===
        let lossy_size = measure_total_lossy_size(&output_manager, &folders);
        let lossless_duration: f64 = folders
            .iter()
            .flat_map(|f| &f.audio_files)
            .filter(|f| !f.is_lossy)
            .map(|f| f.duration)
            .sum();

        let lossless_bitrate = {
            let manual = *state.manual_bitrate.lock().unwrap();
            if let Some(br) = manual {
                br
            } else {
                calculate_optimal_bitrate(lossy_size, lossless_duration)
            }
        };

        let old_bitrate = state.lossless_bitrate.load(Ordering::SeqCst);
        state.lossless_bitrate.store(lossless_bitrate, Ordering::SeqCst);

        println!(
            "Bitrate calculation: lossy_size={} MB, lossless_duration={:.0}s, bitrate={}",
            lossy_size / 1_000_000,
            lossless_duration,
            lossless_bitrate
        );

        // If bitrate changed, delete old lossless outputs
        if old_bitrate != lossless_bitrate {
            println!("Bitrate changed {} -> {}, deleting old lossless outputs", old_bitrate, lossless_bitrate);
            delete_lossless_outputs(&output_manager, &folders);
        }

        // === PHASE 2: Lossless files ===
        state.set_phase(EncodingPhase::LosslessPass);
        let _ = progress_tx.send(EncoderEvent::PhaseTransition {
            phase: EncodingPhase::LosslessPass,
            measured_lossy_size: lossy_size,
            optimal_bitrate: lossless_bitrate,
        });

        for (_folder_idx, folder) in folders.iter().enumerate() {
            if state.is_restart_requested() {
                restart_needed = true;
                break;
            }

            let lossless_files: Vec<&AudioFileInfo> =
                folder.audio_files.iter().filter(|f| !f.is_lossy).collect();

            if lossless_files.is_empty() {
                continue;
            }

            let _ = progress_tx.send(EncoderEvent::FolderStarted {
                id: folder.id.clone(),
                files_total: lossless_files.len(),
            });

            *state.current_folder.lock().unwrap() = Some(folder.id.clone());
            *state.current_progress.lock().unwrap() = (0, lossless_files.len());

            let output_dir = match output_manager.get_folder_output_dir(&folder.id) {
                Ok(dir) => dir,
                Err(e) => {
                    eprintln!("Failed to get output dir for {:?}: {}", folder.id, e);
                    continue;
                }
            };
            let embed_art = state.embed_album_art.load(Ordering::SeqCst);
            let album_art = if embed_art {
                folder.album_art.clone()
            } else {
                None
            };

            for (file_idx, file) in lossless_files.iter().enumerate() {
                if state.is_restart_requested() {
                    restart_needed = true;
                    break;
                }

                let output_path = get_output_path(&output_dir, &file.path);

                // Skip if already encoded
                if output_path.exists() {
                    *state.current_progress.lock().unwrap() = (file_idx + 1, lossless_files.len());
                    continue;
                }

                // Encode at calculated lossless bitrate
                if let Err(e) = transcode_file(
                    &ffmpeg_path,
                    &file.path,
                    &output_path,
                    lossless_bitrate,
                    album_art.as_ref().map(|s| Path::new(s.as_str())),
                ) {
                    eprintln!("Failed to encode {:?}: {}", file.path, e);
                }

                *state.current_progress.lock().unwrap() = (file_idx + 1, lossless_files.len());
                let _ = progress_tx.send(EncoderEvent::FolderProgress {
                    id: folder.id.clone(),
                    files_completed: file_idx + 1,
                    files_total: lossless_files.len(),
                });
            }

            if restart_needed {
                break;
            }

            let output_size = output_manager.get_folder_output_size(&folder.id).unwrap_or(0);
            let _ = progress_tx.send(EncoderEvent::FolderCompleted {
                id: folder.id.clone(),
                output_dir: output_dir.clone(),
                output_size,
                lossless_bitrate: Some(lossless_bitrate),
            });
        }

        if restart_needed {
            println!("Restart requested during lossless pass");
            continue;
        }

        // === ALL COMPLETE ===
        state.set_phase(EncodingPhase::Complete);
        *state.current_folder.lock().unwrap() = None;
        *state.current_progress.lock().unwrap() = (0, 0);

        println!("Encoding complete!");

        // Wait for restart signal or new folders
        loop {
            if state.is_restart_requested() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Get output path for a source file
fn get_output_path(output_dir: &Path, source_path: &Path) -> PathBuf {
    let stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    output_dir.join(format!("{}.mp3", stem))
}

/// Measure total size of lossy file outputs
fn measure_total_lossy_size(output_manager: &OutputManager, folders: &[MusicFolder]) -> u64 {
    folders
        .iter()
        .filter(|f| f.audio_files.iter().any(|af| af.is_lossy))
        .map(|f| output_manager.get_folder_output_size(&f.id).unwrap_or(0))
        .sum()
}

/// Calculate optimal bitrate for lossless files
fn calculate_optimal_bitrate(lossy_size: u64, lossless_duration: f64) -> u32 {
    const CD_CAPACITY: u64 = 700 * 1000 * 1000;
    const SAFETY_MARGIN: f64 = 0.98;

    if lossless_duration <= 0.0 {
        return 320;
    }

    let remaining_space = ((CD_CAPACITY as f64 * SAFETY_MARGIN) as u64).saturating_sub(lossy_size);
    let bitrate = ((remaining_space * 8) as f64 / lossless_duration / 1000.0) as u32;
    bitrate.clamp(64, 320)
}

/// Delete lossless-sourced outputs (when bitrate changes)
fn delete_lossless_outputs(output_manager: &OutputManager, folders: &[MusicFolder]) {
    for folder in folders {
        // Only delete if folder has lossless files
        if folder.audio_files.iter().any(|f| !f.is_lossy) {
            let _ = output_manager.delete_folder_output_from_session(&folder.id);
        }
    }
}

/// Simple synchronous file transcoding using ffmpeg
fn transcode_file(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    bitrate: u32,
    album_art_path: Option<&Path>,
) -> Result<(), String> {
    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create output dir: {}", e))?;
        }
    }

    let bitrate_str = format!("{}k", bitrate);

    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-y") // Overwrite output
        .arg("-i")
        .arg(input_path)
        .arg("-vn") // No video
        .arg("-codec:a")
        .arg("libmp3lame")
        .arg("-b:a")
        .arg(&bitrate_str)
        .arg("-map_metadata")
        .arg("0") // Copy metadata
        .arg("-id3v2_version")
        .arg("3");

    // Add album art if provided
    if let Some(art_path) = album_art_path {
        if art_path.exists() {
            cmd.arg("-i")
                .arg(art_path)
                .arg("-map")
                .arg("0:a")
                .arg("-map")
                .arg("1:v")
                .arg("-c:v")
                .arg("copy")
                .arg("-metadata:s:v")
                .arg("title=Album cover")
                .arg("-metadata:s:v")
                .arg("comment=Cover (front)");
        }
    }

    cmd.arg(output_path);

    let output = cmd.output().map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("ffmpeg failed: {}", stderr))
    }
}
