//! Tests for FolderList component

use super::*;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

#[test]
fn test_folder_list_new() {
    let list = FolderList::new_for_test();
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
}

#[test]
fn test_add_folder() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/folder1"));
    list.folders.push(MusicFolder::new_for_test("/test/folder2"));

    assert_eq!(list.len(), 2);
}

#[test]
fn test_remove_folder() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/folder1"));
    list.folders.push(MusicFolder::new_for_test("/test/folder2"));

    list.remove_folder(0);

    assert_eq!(list.len(), 1);
    assert_eq!(list.folders[0].path, PathBuf::from("/test/folder2"));
}

#[test]
fn test_move_folder_forward() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/a"));
    list.folders.push(MusicFolder::new_for_test("/test/b"));
    list.folders.push(MusicFolder::new_for_test("/test/c"));

    // Move "a" to position 2 (after "b")
    list.move_folder(0, 2);

    assert_eq!(list.folders[0].path, PathBuf::from("/test/b"));
    assert_eq!(list.folders[1].path, PathBuf::from("/test/a"));
    assert_eq!(list.folders[2].path, PathBuf::from("/test/c"));
}

#[test]
fn test_move_folder_backward() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/a"));
    list.folders.push(MusicFolder::new_for_test("/test/b"));
    list.folders.push(MusicFolder::new_for_test("/test/c"));

    // Move "c" to position 0 (before "a")
    list.move_folder(2, 0);

    assert_eq!(list.folders[0].path, PathBuf::from("/test/c"));
    assert_eq!(list.folders[1].path, PathBuf::from("/test/a"));
    assert_eq!(list.folders[2].path, PathBuf::from("/test/b"));
}

#[test]
fn test_clear() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/folder1"));
    list.folders.push(MusicFolder::new_for_test("/test/folder2"));

    list.clear();

    assert!(list.is_empty());
}

#[test]
fn test_total_files() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/folder1")); // 10 files
    list.folders.push(MusicFolder::new_for_test("/test/folder2")); // 10 files

    assert_eq!(list.total_files(), 20);
}

#[test]
fn test_total_size() {
    let mut list = FolderList::new_for_test();
    list.folders.push(MusicFolder::new_for_test("/test/folder1")); // 50MB
    list.folders.push(MusicFolder::new_for_test("/test/folder2")); // 50MB

    assert_eq!(list.total_size(), 100_000_000);
}

// ConversionState tests

#[test]
fn test_conversion_state_new() {
    let state = ConversionState::new();

    assert!(!state.is_converting());
    let (completed, failed, total) = state.progress();
    assert_eq!(completed, 0);
    assert_eq!(failed, 0);
    assert_eq!(total, 0);
}

#[test]
fn test_conversion_state_reset() {
    let state = ConversionState::new();

    state.reset(24);

    assert!(state.is_converting());
    let (completed, failed, total) = state.progress();
    assert_eq!(completed, 0);
    assert_eq!(failed, 0);
    assert_eq!(total, 24);
}

#[test]
fn test_conversion_state_finish() {
    let state = ConversionState::new();
    state.reset(10);
    assert!(state.is_converting());

    state.finish();

    assert!(!state.is_converting());
}

#[test]
fn test_conversion_state_progress_updates() {
    let state = ConversionState::new();
    state.reset(5);

    // Simulate completing some files
    state.completed.fetch_add(1, Ordering::SeqCst);
    state.completed.fetch_add(1, Ordering::SeqCst);
    state.failed.fetch_add(1, Ordering::SeqCst);

    let (completed, failed, total) = state.progress();
    assert_eq!(completed, 2);
    assert_eq!(failed, 1);
    assert_eq!(total, 5);
}

#[test]
fn test_conversion_state_clone_shares_atomics() {
    let state1 = ConversionState::new();
    state1.reset(10);

    let state2 = state1.clone();

    // Update via state1
    state1.completed.fetch_add(5, Ordering::SeqCst);

    // Should be visible via state2 (shared Arc)
    let (completed, _, _) = state2.progress();
    assert_eq!(completed, 5);
}

#[test]
fn test_conversion_state_thread_safety() {
    use std::thread;

    let state = ConversionState::new();
    state.reset(100);

    let mut handles = vec![];

    // Spawn 10 threads, each incrementing completed 10 times
    for _ in 0..10 {
        let state_clone = state.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..10 {
                state_clone.completed.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let (completed, _, _) = state.progress();
    assert_eq!(completed, 100);
}

#[test]
fn test_conversion_state_cancellation() {
    let state = ConversionState::new();
    state.reset(10);

    // Initially not cancelled
    assert!(!state.is_cancelled());

    // Request cancel
    state.request_cancel();

    // Should now be cancelled
    assert!(state.is_cancelled());
    // But should still be converting (in-flight tasks finish)
    assert!(state.is_converting());
}

#[test]
fn test_conversion_state_reset_clears_cancel() {
    let state = ConversionState::new();
    state.reset(10);
    state.request_cancel();
    assert!(state.is_cancelled());

    // Reset should clear the cancel flag
    state.reset(5);
    assert!(!state.is_cancelled());
    assert!(state.is_converting());
}
