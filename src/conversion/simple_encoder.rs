//! Simplified Background Encoder
//!
//! Core principle: Folder list is source of truth. Encoding is stateless and restartable.
//! When anything changes → restart fresh. Use file existence to skip done work.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    /// PIDs of currently running ffmpeg processes (for instant termination)
    running_pids: Mutex<HashSet<u32>>,
}

impl SimpleEncoderState {
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(EncodingPhase::Idle),
            lossless_bitrate: AtomicU32::new(0), // 0 = not yet calculated
            restart_requested: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            embed_album_art: AtomicBool::new(false),
            current_folder: Mutex::new(None),
            current_progress: Mutex::new((0, 0)),
            manual_bitrate: Mutex::new(None),
            running_pids: Mutex::new(HashSet::new()),
        }
    }

    pub fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::SeqCst);
        // Kill any running ffmpeg processes for instant response
        self.kill_running_processes();
    }

    /// Register a running ffmpeg process PID
    pub fn register_pid(&self, pid: u32) {
        self.running_pids.lock().unwrap().insert(pid);
    }

    /// Unregister a ffmpeg process PID (when it completes)
    pub fn unregister_pid(&self, pid: u32) {
        self.running_pids.lock().unwrap().remove(&pid);
    }

    /// Kill all running ffmpeg processes (for instant restart)
    pub fn kill_running_processes(&self) {
        let pids: Vec<u32> = self.running_pids.lock().unwrap().iter().copied().collect();
        for pid in pids {
            #[cfg(unix)]
            unsafe {
                // SIGKILL for immediate termination
                libc::kill(pid as i32, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            {
                // On non-Unix, we can't easily kill by PID
                // The process will complete and we'll restart after
                let _ = pid;
            }
        }
        // Clear the set (processes are dead or will be soon)
        self.running_pids.lock().unwrap().clear();
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
                    &state,
                ) {
                    // Don't log if killed due to restart
                    if !state.is_restart_requested() {
                        eprintln!("Failed to encode {:?}: {}", file.path, e);
                    }
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

        // If bitrate changed, delete old lossless outputs and notify UI
        if old_bitrate != 0 && old_bitrate != lossless_bitrate {
            // old_bitrate == 0 means this is the first calculation (no previous encoding)
            println!("Bitrate changed {} -> {}, deleting old lossless outputs", old_bitrate, lossless_bitrate);
            delete_lossless_outputs(&output_manager, &folders);

            // Collect folders with lossless files that need re-encoding
            let reencode_needed: Vec<FolderId> = folders
                .iter()
                .filter(|f| f.audio_files.iter().any(|a| !a.is_lossy))
                .map(|f| f.id.clone())
                .collect();

            if !reencode_needed.is_empty() {
                let _ = progress_tx.send(EncoderEvent::BitrateRecalculated {
                    new_bitrate: lossless_bitrate,
                    reencode_needed,
                });
            }
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

            // Encode lossless files in parallel
            let folder_id = folder.id.clone();
            let progress_tx_clone = progress_tx.clone();
            let total_files = lossless_files.len();
            let state_for_progress = state.clone();

            let (_completed, was_interrupted) = encode_files_parallel(
                &lossless_files,
                &output_dir,
                lossless_bitrate,
                album_art.as_deref(),
                &ffmpeg_path,
                &state,
                move |count| {
                    *state_for_progress.current_progress.lock().unwrap() = (count, total_files);
                    let _ = progress_tx_clone.send(EncoderEvent::FolderProgress {
                        id: folder_id.clone(),
                        files_completed: count,
                        files_total: total_files,
                    });
                },
            );

            if was_interrupted {
                restart_needed = true;
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

/// Simple synchronous file transcoding using ffmpeg with PID tracking for instant termination
fn transcode_file(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    bitrate: u32,
    album_art_path: Option<&Path>,
    state: &SimpleEncoderState,
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

    // Suppress ffmpeg output
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    // Spawn the process so we can track its PID
    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;
    let pid = child.id();

    // Register PID for instant termination capability
    state.register_pid(pid);

    // Wait for completion (or termination via SIGKILL)
    let status = child.wait().map_err(|e| format!("Failed to wait for ffmpeg: {}", e))?;

    // Unregister PID
    state.unregister_pid(pid);

    if status.success() {
        Ok(())
    } else {
        // Check if we were killed (exit code will be non-zero)
        // Delete partial output file to avoid corruption on re-encode
        let _ = std::fs::remove_file(output_path);

        if state.is_restart_requested() {
            Err("Process terminated due to restart".to_string())
        } else {
            Err(format!("ffmpeg failed with status: {}", status))
        }
    }
}

/// Calculate optimal worker count based on CPU cores
fn calculate_worker_count() -> usize {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    // Use 75% of cores, clamped between 2 and 8
    ((available as f32 * 0.75).ceil() as usize).clamp(2, 8)
}

/// Job for parallel encoding
struct EncodeJob {
    input_path: PathBuf,
    output_path: PathBuf,
    bitrate: u32,
    album_art: Option<String>,
}

/// Encode files in parallel with PID tracking for instant termination
///
/// Returns (completed_count, was_interrupted)
fn encode_files_parallel(
    files: &[&AudioFileInfo],
    output_dir: &Path,
    bitrate: u32,
    album_art: Option<&str>,
    ffmpeg_path: &Path,
    state: &Arc<SimpleEncoderState>,
    progress_callback: impl Fn(usize) + Send + Sync + 'static,
) -> (usize, bool) {
    use std::sync::atomic::AtomicUsize;

    let worker_count = calculate_worker_count();
    let completed = Arc::new(AtomicUsize::new(0));
    let total = files.len();

    // Create job queue
    let jobs: Vec<EncodeJob> = files
        .iter()
        .map(|file| EncodeJob {
            input_path: file.path.clone(),
            output_path: get_output_path(output_dir, &file.path),
            bitrate,
            album_art: album_art.map(|s| s.to_string()),
        })
        .collect();

    // Filter to only jobs that need encoding (output doesn't exist)
    let pending_jobs: Vec<EncodeJob> = jobs
        .into_iter()
        .filter(|job| !job.output_path.exists())
        .collect();

    // Count already-complete files
    let already_done = total - pending_jobs.len();
    completed.store(already_done, Ordering::SeqCst);

    if pending_jobs.is_empty() {
        return (total, false);
    }

    println!(
        "Parallel encoding: {} files with {} workers ({} already done)",
        pending_jobs.len(),
        worker_count,
        already_done
    );

    // Create work channel
    let (job_tx, job_rx) = std::sync::mpsc::channel::<EncodeJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));

    // Spawn worker threads
    let mut handles = Vec::new();
    let ffmpeg_path = ffmpeg_path.to_path_buf();
    let progress_callback = Arc::new(progress_callback);

    for worker_id in 0..worker_count {
        let job_rx = job_rx.clone();
        let state = state.clone();
        let completed = completed.clone();
        let ffmpeg_path = ffmpeg_path.clone();
        let progress_callback = progress_callback.clone();

        let handle = thread::spawn(move || {
            loop {
                // Check for restart before taking a job
                if state.is_restart_requested() {
                    break;
                }

                // Try to get a job
                let job = {
                    let rx = job_rx.lock().unwrap();
                    rx.try_recv().ok()
                };

                let job = match job {
                    Some(j) => j,
                    None => {
                        // No more jobs, check if channel is disconnected
                        thread::sleep(Duration::from_millis(10));
                        let rx = job_rx.lock().unwrap();
                        match rx.try_recv() {
                            Ok(j) => j,
                            Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        }
                    }
                };

                // Encode the file
                let result = transcode_file_internal(
                    &ffmpeg_path,
                    &job.input_path,
                    &job.output_path,
                    job.bitrate,
                    job.album_art.as_ref().map(|s| Path::new(s.as_str())),
                    &state,
                );

                if let Err(e) = result {
                    if !state.is_restart_requested() {
                        eprintln!("[Worker {}] Failed: {:?} - {}", worker_id, job.input_path, e);
                    }
                }

                // Update progress
                let count = completed.fetch_add(1, Ordering::SeqCst) + 1;
                progress_callback(count);
            }
        });

        handles.push(handle);
    }

    // Send all jobs to workers
    for job in pending_jobs {
        if state.is_restart_requested() {
            break;
        }
        let _ = job_tx.send(job);
    }

    // Drop sender to signal workers to finish
    drop(job_tx);

    // Wait for all workers to complete
    for handle in handles {
        let _ = handle.join();
    }

    let final_count = completed.load(Ordering::SeqCst);
    let was_interrupted = state.is_restart_requested();

    (final_count, was_interrupted)
}

/// Internal transcode function that takes state by reference (for parallel use)
fn transcode_file_internal(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    bitrate: u32,
    album_art_path: Option<&Path>,
    state: &SimpleEncoderState,
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
    cmd.arg("-y")
        .arg("-i")
        .arg(input_path)
        .arg("-vn")
        .arg("-codec:a")
        .arg("libmp3lame")
        .arg("-b:a")
        .arg(&bitrate_str)
        .arg("-map_metadata")
        .arg("0")
        .arg("-id3v2_version")
        .arg("3");

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
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;
    let pid = child.id();

    state.register_pid(pid);
    let status = child.wait().map_err(|e| format!("Failed to wait for ffmpeg: {}", e))?;
    state.unregister_pid(pid);

    if status.success() {
        Ok(())
    } else if state.is_restart_requested() {
        // Process was killed - delete partial output file to avoid corruption
        let _ = std::fs::remove_file(output_path);
        Err("Process terminated due to restart".to_string())
    } else {
        // Process failed normally - also delete partial output
        let _ = std::fs::remove_file(output_path);
        Err(format!("ffmpeg failed with status: {}", status))
    }
}
