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

use crate::audio::{determine_encoding_strategy, EncodingStrategy};
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
        // First, stop any running encoding processes
        self.state.request_restart(); // This sets restart flag AND kills running ffmpeg processes

        // Wait for processes to actually terminate
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Update state (but DON'T clear restart flag - let encoding loop see it)
        self.state.set_phase(EncodingPhase::Idle);
        *self.state.current_folder.lock().unwrap() = None;
        *self.state.current_progress.lock().unwrap() = (0, 0);
        *self.state.manual_bitrate.lock().unwrap() = None;
        // Reset lossless bitrate so we don't compare against stale value
        self.state.lossless_bitrate.store(0, Ordering::SeqCst);

        // Clean up output files
        if let Err(e) = self.output_manager.cleanup() {
            eprintln!("Warning: Failed to cleanup output files: {}", e);
        }
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

    /// Add or update a folder (updates shared folders and restarts)
    ///
    /// If a folder with the same ID exists, it is replaced with the new one.
    /// This allows updating exclusions, track order, etc.
    pub fn add_folder(&self, folder: MusicFolder) {
        let mut guard = self.shared_folders.lock().unwrap();
        // Replace existing folder with same ID, or add new
        if let Some(existing) = guard.iter_mut().find(|f| f.id == folder.id) {
            *existing = folder;
        } else {
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

    /// Recalculate bitrate with optional manual override
    pub fn recalculate_bitrate(&self, target: u32) {
        // Set manual bitrate override (0 means auto-calculate)
        if target == 0 {
            *self.state.manual_bitrate.lock().unwrap() = None;
        } else {
            *self.state.manual_bitrate.lock().unwrap() = Some(target);
        }
        self.restart();
    }

    /// Clear all state (compatibility - alias for clear())
    pub fn clear_all(&self) {
        // IMPORTANT: Clear folders FIRST so encoding loop sees empty list
        // and doesn't restart encoding after we clean up files
        self.shared_folders.lock().unwrap().clear();
        // Now clear state and delete files
        self.clear();
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

        // === PHASE 1: Lossy files (global parallel encoding with smart strategies) ===
        state.set_phase(EncodingPhase::LossyPass);
        let _ = progress_tx.send(EncoderEvent::PhaseTransition {
            phase: EncodingPhase::LossyPass,
            measured_lossy_size: 0,
            optimal_bitrate: 320,
        });

        let embed_art = state.embed_album_art.load(Ordering::SeqCst);
        let was_interrupted = encode_all_lossy_parallel(
            &folders,
            320, // Target bitrate (used for strategy decisions)
            &ffmpeg_path,
            &output_manager,
            &state,
            embed_art,
            &progress_tx,
        );

        if was_interrupted {
            println!("Restart requested during lossy pass");
            continue;
        }

        // === MEASURE & CALCULATE BITRATE ===
        let lossy_size = measure_total_lossy_size(&output_manager, &folders);
        // Use active_tracks() to respect exclusions and custom order
        let lossless_duration: f64 = folders
            .iter()
            .flat_map(|f| f.active_tracks())
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

            // Collect folders with active lossless files that need re-encoding
            let reencode_needed: Vec<FolderId> = folders
                .iter()
                .filter(|f| f.active_tracks().iter().any(|a| !a.is_lossy))
                .map(|f| f.id.clone())
                .collect();

            if !reencode_needed.is_empty() {
                let _ = progress_tx.send(EncoderEvent::BitrateRecalculated {
                    new_bitrate: lossless_bitrate,
                    reencode_needed,
                });
            }
        }

        // === PHASE 2: Lossless files (global parallel encoding) ===
        state.set_phase(EncodingPhase::LosslessPass);
        let _ = progress_tx.send(EncoderEvent::PhaseTransition {
            phase: EncodingPhase::LosslessPass,
            measured_lossy_size: lossy_size,
            optimal_bitrate: lossless_bitrate,
        });

        let embed_art = state.embed_album_art.load(Ordering::SeqCst);
        let was_interrupted = encode_all_lossless_parallel(
            &folders,
            lossless_bitrate,
            &ffmpeg_path,
            &output_manager,
            &state,
            embed_art,
            &progress_tx,
        );

        if was_interrupted {
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
///
/// Output files are named after the source file stem with .mp3 extension.
/// Numbered prefixes for track ordering are applied during ISO staging.
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
        .filter(|f| f.active_tracks().iter().any(|af| af.is_lossy))
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

    let usable_capacity = (CD_CAPACITY as f64 * SAFETY_MARGIN) as u64;
    let remaining_space = usable_capacity.saturating_sub(lossy_size);
    let bitrate = ((remaining_space * 8) as f64 / lossless_duration / 1000.0) as u32;

    bitrate.clamp(64, 320)
}

/// Delete lossless-sourced outputs (when bitrate changes)
fn delete_lossless_outputs(output_manager: &OutputManager, folders: &[MusicFolder]) {
    for folder in folders {
        // Only delete if folder has active lossless files (respects exclusions)
        if folder.active_tracks().iter().any(|f| !f.is_lossy) {
            let _ = output_manager.delete_folder_output_from_session(&folder.id);
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

/// Job for global parallel encoding (across all folders)
struct GlobalEncodeJob {
    folder_id: FolderId,
    input_path: PathBuf,
    output_path: PathBuf,
    bitrate: u32,
    album_art: Option<String>,
}

/// Folder context for global encoding
struct FolderContext {
    output_dir: PathBuf,
    total_files: usize,
}

/// Job for lossy encoding with smart strategy (Copy/Transcode)
struct LossyEncodeJob {
    folder_id: FolderId,
    input_path: PathBuf,
    output_path: PathBuf,
    strategy: EncodingStrategy,
    album_art: Option<String>,
}

/// Encode ALL lossless files from ALL folders in a single parallel pool
///
/// This keeps all workers busy until everything is done, rather than
/// draining the pool between folders.
///
/// Returns true if interrupted by restart
fn encode_all_lossless_parallel(
    folders: &[MusicFolder],
    bitrate: u32,
    ffmpeg_path: &Path,
    output_manager: &OutputManager,
    state: &Arc<SimpleEncoderState>,
    embed_album_art: bool,
    progress_tx: &mpsc::Sender<EncoderEvent>,
) -> bool {
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;

    let worker_count = calculate_worker_count();

    // Build folder contexts and collect all jobs
    let mut folder_contexts: HashMap<FolderId, FolderContext> = HashMap::new();
    let mut all_jobs: Vec<GlobalEncodeJob> = Vec::new();
    let mut folder_completed: HashMap<FolderId, Arc<AtomicUsize>> = HashMap::new();

    for folder in folders {
        // Get all active tracks with their original indices (for correct numbering)
        let all_tracks: Vec<(usize, &AudioFileInfo)> = folder
            .active_tracks()
            .into_iter()
            .enumerate()
            .collect();

        // Filter to lossless files, keeping original index
        let lossless_files: Vec<(usize, &AudioFileInfo)> = all_tracks
            .iter()
            .filter(|(_, f)| !f.is_lossy)
            .cloned()
            .collect();

        if lossless_files.is_empty() {
            continue;
        }

        let output_dir = match output_manager.get_folder_output_dir(&folder.id) {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Failed to get output dir for {:?}: {}", folder.id, e);
                continue;
            }
        };

        let album_art = if embed_album_art {
            folder.album_art.clone()
        } else {
            None
        };

        // Store folder context
        folder_contexts.insert(
            folder.id.clone(),
            FolderContext {
                output_dir: output_dir.clone(),
                total_files: lossless_files.len(),
            },
        );

        // Initialize completed counter for this folder
        folder_completed.insert(folder.id.clone(), Arc::new(AtomicUsize::new(0)));

        // Create jobs for all files in this folder
        // Note: Numbered prefixes are applied during ISO staging, not here
        for (_original_idx, file) in &lossless_files {
            let output_path = get_output_path(&output_dir, &file.path);

            // Skip already-encoded files
            if output_path.exists() {
                folder_completed
                    .get(&folder.id)
                    .unwrap()
                    .fetch_add(1, Ordering::SeqCst);
                continue;
            }

            all_jobs.push(GlobalEncodeJob {
                folder_id: folder.id.clone(),
                input_path: file.path.clone(),
                output_path,
                bitrate,
                album_art: album_art.clone(),
            });
        }

        // Send FolderStarted event
        let _ = progress_tx.send(EncoderEvent::FolderStarted {
            id: folder.id.clone(),
            files_total: lossless_files.len(),
        });

        // Check if this folder is already complete (all files existed)
        let completed_count = folder_completed
            .get(&folder.id)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0);

        if completed_count >= lossless_files.len() {
            // Send completion event immediately for this folder
            let output_size = output_manager.get_folder_output_size(&folder.id).unwrap_or(0);
            let _ = progress_tx.send(EncoderEvent::FolderCompleted {
                id: folder.id.clone(),
                output_dir: output_dir.clone(),
                output_size,
                lossless_bitrate: Some(bitrate),
            });
        }
    }

    if all_jobs.is_empty() {
        // All folders were already complete
        return false;
    }

    let total_jobs = all_jobs.len();
    println!(
        "Global parallel encoding: {} files across {} folders with {} workers",
        total_jobs,
        folder_contexts.len(),
        worker_count
    );

    // Create work channel
    let (job_tx, job_rx) = std::sync::mpsc::channel::<GlobalEncodeJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));

    // Shared state for tracking folder completion
    let folder_completed = Arc::new(folder_completed);
    let folder_contexts = Arc::new(folder_contexts);

    // Track which folders have been marked complete
    let folders_finished: Arc<Mutex<HashSet<FolderId>>> = Arc::new(Mutex::new(HashSet::new()));

    // Spawn worker threads
    let mut handles = Vec::new();
    let ffmpeg_path = ffmpeg_path.to_path_buf();

    for _worker_id in 0..worker_count {
        let job_rx = job_rx.clone();
        let state = state.clone();
        let ffmpeg_path = ffmpeg_path.clone();
        let folder_completed = folder_completed.clone();
        let folder_contexts = folder_contexts.clone();
        let folders_finished = folders_finished.clone();
        let progress_tx = progress_tx.clone();
        let _output_manager_session = output_manager.session_id().to_string();

        let handle = thread::spawn(move || {
            loop {
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
                        thread::sleep(Duration::from_millis(10));
                        let rx = job_rx.lock().unwrap();
                        match rx.try_recv() {
                            Ok(j) => j,
                            Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        }
                    }
                };

                let folder_id = job.folder_id.clone();

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
                        eprintln!("Failed to encode {:?}: {}", job.input_path, e);
                    }
                }

                // Update folder progress
                if let Some(counter) = folder_completed.get(&folder_id) {
                    let completed = counter.fetch_add(1, Ordering::SeqCst) + 1;

                    if let Some(ctx) = folder_contexts.get(&folder_id) {
                        // Send progress event
                        let _ = progress_tx.send(EncoderEvent::FolderProgress {
                            id: folder_id.clone(),
                            files_completed: completed,
                            files_total: ctx.total_files,
                        });

                        // Check if folder is complete
                        if completed >= ctx.total_files {
                            let mut finished = folders_finished.lock().unwrap();
                            if !finished.contains(&folder_id) {
                                finished.insert(folder_id.clone());

                                // Calculate output size
                                let output_size = std::fs::read_dir(&ctx.output_dir)
                                    .map(|entries| {
                                        entries
                                            .filter_map(|e| e.ok())
                                            .filter_map(|e| e.metadata().ok())
                                            .map(|m| m.len())
                                            .sum()
                                    })
                                    .unwrap_or(0);

                                let _ = progress_tx.send(EncoderEvent::FolderCompleted {
                                    id: folder_id.clone(),
                                    output_dir: ctx.output_dir.clone(),
                                    output_size,
                                    lossless_bitrate: Some(job.bitrate),
                                });
                            }
                        }
                    }
                }
            }
        });

        handles.push(handle);
    }

    // Send all jobs to workers
    for job in all_jobs {
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

    state.is_restart_requested()
}

/// Encode ALL lossy files from ALL folders in a single parallel pool
///
/// Uses smart encoding strategies:
/// - MP3s near target bitrate: Copy (no re-encoding)
/// - Other lossy (AAC, OGG): Transcode at source bitrate
///
/// Returns true if interrupted by restart
fn encode_all_lossy_parallel(
    folders: &[MusicFolder],
    target_bitrate: u32,
    ffmpeg_path: &Path,
    output_manager: &OutputManager,
    state: &Arc<SimpleEncoderState>,
    embed_album_art: bool,
    progress_tx: &mpsc::Sender<EncoderEvent>,
) -> bool {
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;

    let worker_count = calculate_worker_count();

    // Build folder contexts and collect all jobs
    let mut folder_contexts: HashMap<FolderId, FolderContext> = HashMap::new();
    let mut all_jobs: Vec<LossyEncodeJob> = Vec::new();
    let mut folder_completed: HashMap<FolderId, Arc<AtomicUsize>> = HashMap::new();

    for folder in folders {
        // Get all active tracks with their original indices (for correct numbering)
        let all_tracks: Vec<(usize, &AudioFileInfo)> = folder
            .active_tracks()
            .into_iter()
            .enumerate()
            .collect();

        // Filter to lossy files, keeping original index
        let lossy_files: Vec<(usize, &AudioFileInfo)> = all_tracks
            .iter()
            .filter(|(_, f)| f.is_lossy)
            .cloned()
            .collect();

        if lossy_files.is_empty() {
            continue;
        }

        let output_dir = match output_manager.get_folder_output_dir(&folder.id) {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Failed to get output dir for {:?}: {}", folder.id, e);
                continue;
            }
        };

        let album_art = if embed_album_art {
            folder.album_art.clone()
        } else {
            None
        };

        // Store folder context
        folder_contexts.insert(
            folder.id.clone(),
            FolderContext {
                output_dir: output_dir.clone(),
                total_files: lossy_files.len(),
            },
        );

        // Initialize completed counter for this folder
        folder_completed.insert(folder.id.clone(), Arc::new(AtomicUsize::new(0)));

        // Create jobs for all files in this folder with smart strategies
        // Note: Numbered prefixes are applied during ISO staging, not here
        for (_original_idx, file) in &lossy_files {
            let output_path = get_output_path(&output_dir, &file.path);

            // Skip already-encoded files
            if output_path.exists() {
                folder_completed
                    .get(&folder.id)
                    .unwrap()
                    .fetch_add(1, Ordering::SeqCst);
                continue;
            }

            // Determine encoding strategy for this file
            let strategy = determine_encoding_strategy(
                &file.codec,
                file.bitrate,
                target_bitrate,
                file.is_lossy,
                false, // no_lossy_mode - we're not implementing this yet
                embed_album_art,
            );

            all_jobs.push(LossyEncodeJob {
                folder_id: folder.id.clone(),
                input_path: file.path.clone(),
                output_path,
                strategy,
                album_art: album_art.clone(),
            });
        }

        // Send FolderStarted event
        let _ = progress_tx.send(EncoderEvent::FolderStarted {
            id: folder.id.clone(),
            files_total: lossy_files.len(),
        });

        // Check if this folder is already complete (all files existed)
        let completed_count = folder_completed
            .get(&folder.id)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0);

        if completed_count >= lossy_files.len() {
            // Send completion event immediately for this folder
            let output_size = output_manager.get_folder_output_size(&folder.id).unwrap_or(0);
            let _ = progress_tx.send(EncoderEvent::FolderCompleted {
                id: folder.id.clone(),
                output_dir: output_dir.clone(),
                output_size,
                lossless_bitrate: None,
            });
        }
    }

    if all_jobs.is_empty() {
        // All folders were already complete
        return false;
    }

    // Count strategies for logging
    let copy_count = all_jobs.iter().filter(|j| matches!(j.strategy, EncodingStrategy::Copy | EncodingStrategy::CopyWithoutArt)).count();
    let transcode_count = all_jobs.len() - copy_count;

    println!(
        "Global parallel lossy encoding: {} files ({} copy, {} transcode) across {} folders with {} workers",
        all_jobs.len(),
        copy_count,
        transcode_count,
        folder_contexts.len(),
        worker_count
    );

    // Create work channel
    let (job_tx, job_rx) = std::sync::mpsc::channel::<LossyEncodeJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));

    // Shared state for tracking folder completion
    let folder_completed = Arc::new(folder_completed);
    let folder_contexts = Arc::new(folder_contexts);

    // Track which folders have been marked complete
    let folders_finished: Arc<Mutex<HashSet<FolderId>>> = Arc::new(Mutex::new(HashSet::new()));

    // Spawn worker threads
    let mut handles = Vec::new();
    let ffmpeg_path = ffmpeg_path.to_path_buf();

    for _worker_id in 0..worker_count {
        let job_rx = job_rx.clone();
        let state = state.clone();
        let ffmpeg_path = ffmpeg_path.clone();
        let folder_completed = folder_completed.clone();
        let folder_contexts = folder_contexts.clone();
        let folders_finished = folders_finished.clone();
        let progress_tx = progress_tx.clone();

        let handle = thread::spawn(move || {
            loop {
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
                        thread::sleep(Duration::from_millis(10));
                        let rx = job_rx.lock().unwrap();
                        match rx.try_recv() {
                            Ok(j) => j,
                            Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        }
                    }
                };

                let folder_id = job.folder_id.clone();

                // Execute the encoding strategy
                let result = execute_encoding_strategy(
                    &ffmpeg_path,
                    &job.input_path,
                    &job.output_path,
                    &job.strategy,
                    job.album_art.as_ref().map(|s| Path::new(s.as_str())),
                    &state,
                );

                if let Err(e) = result {
                    if !state.is_restart_requested() {
                        eprintln!("Failed to encode {:?}: {}", job.input_path, e);
                    }
                }

                // Update folder progress
                if let Some(counter) = folder_completed.get(&folder_id) {
                    let completed = counter.fetch_add(1, Ordering::SeqCst) + 1;

                    if let Some(ctx) = folder_contexts.get(&folder_id) {
                        // Send progress event
                        let _ = progress_tx.send(EncoderEvent::FolderProgress {
                            id: folder_id.clone(),
                            files_completed: completed,
                            files_total: ctx.total_files,
                        });

                        // Check if folder is complete
                        if completed >= ctx.total_files {
                            let mut finished = folders_finished.lock().unwrap();
                            if !finished.contains(&folder_id) {
                                finished.insert(folder_id.clone());

                                // Calculate output size
                                let output_size = std::fs::read_dir(&ctx.output_dir)
                                    .map(|entries| {
                                        entries
                                            .filter_map(|e| e.ok())
                                            .filter_map(|e| e.metadata().ok())
                                            .map(|m| m.len())
                                            .sum()
                                    })
                                    .unwrap_or(0);

                                let _ = progress_tx.send(EncoderEvent::FolderCompleted {
                                    id: folder_id.clone(),
                                    output_dir: ctx.output_dir.clone(),
                                    output_size,
                                    lossless_bitrate: None,
                                });
                            }
                        }
                    }
                }
            }
        });

        handles.push(handle);
    }

    // Send all jobs to workers
    for job in all_jobs {
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

    state.is_restart_requested()
}

/// Execute an encoding strategy (Copy, CopyWithoutArt, or Transcode)
fn execute_encoding_strategy(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    strategy: &EncodingStrategy,
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

    match strategy {
        EncodingStrategy::Copy => {
            // Direct file copy - preserves everything including album art
            std::fs::copy(input_path, output_path)
                .map_err(|e| format!("Failed to copy file: {}", e))?;
            Ok(())
        }
        EncodingStrategy::CopyWithoutArt => {
            // Use ffmpeg to copy audio stream without album art
            let mut cmd = Command::new(ffmpeg_path);
            cmd.arg("-y")
                .arg("-i")
                .arg(input_path)
                .arg("-vn") // Strip video/album art
                .arg("-codec:a")
                .arg("copy") // Copy audio stream as-is
                .arg("-map_metadata")
                .arg("0")
                .arg(output_path);

            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::piped());

            let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;
            let pid = child.id();

            state.register_pid(pid);
            let status = child.wait().map_err(|e| format!("Failed to wait for ffmpeg: {}", e))?;
            state.unregister_pid(pid);

            if status.success() {
                Ok(())
            } else {
                let _ = std::fs::remove_file(output_path);
                if state.is_restart_requested() {
                    Err("Process terminated due to restart".to_string())
                } else {
                    Err(format!("ffmpeg copy failed with status: {}", status))
                }
            }
        }
        EncodingStrategy::ConvertAtSourceBitrate(bitrate) | EncodingStrategy::ConvertAtTargetBitrate(bitrate) => {
            // Transcode using the internal function
            transcode_file_internal(ffmpeg_path, input_path, output_path, *bitrate, album_art_path, state)
        }
    }
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
