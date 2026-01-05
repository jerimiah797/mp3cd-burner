//! MP3 CD Burner - GPUI Application
//!
//! A native macOS application for converting music folders to MP3
//! and burning them to CD.

mod actions;
mod audio;
mod burning;
mod conversion;
mod core;
mod profiles;
mod ui;

use actions::{
    About, NewProfile, OpenDisplaySettings, OpenOutputDir, OpenProfile, Quit, SaveProfile,
    SetVolumeLabel, ToggleEmbedAlbumArt, ToggleSimulateBurn, push_pending_file,
};
use core::{AppSettings, DisplaySettings, WindowState};
use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, WindowBounds, WindowHandle,
    WindowOptions, point, prelude::*, px, size,
};
use ui::components::{AboutBox, DisplaySettingsModal, FolderList};

/// Decode percent-encoded URL path (e.g., %20 -> space)
fn percent_decode_str(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Build the application menus with current settings state
fn build_menus(settings: &AppSettings) -> Vec<Menu> {
    // Use checkmark prefix when enabled
    let simulate_burn_label = if settings.simulate_burn {
        "✓ Simulate Burn"
    } else {
        "Simulate Burn"
    };

    let embed_album_art_label = if settings.embed_album_art {
        "✓ Embed Album Art"
    } else {
        "Embed Album Art"
    };

    vec![
        Menu {
            name: "MP3 CD Burner".into(),
            items: vec![
                MenuItem::action("About MP3 CD Burner", About),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New", NewProfile),
                MenuItem::action("Open Burn Profile...", OpenProfile),
                MenuItem::separator(),
                MenuItem::action("Save Burn Profile...", SaveProfile),
            ],
        },
        Menu {
            name: "Options".into(),
            items: vec![
                MenuItem::action(simulate_burn_label, ToggleSimulateBurn),
                // TODO: MenuItem::action("No Lossy Conversions", ToggleNoLossyConversions),
                MenuItem::action(embed_album_art_label, ToggleEmbedAlbumArt),
                MenuItem::separator(),
                MenuItem::action("Set CD Volume Label...", SetVolumeLabel),
                MenuItem::action("Display Settings...", OpenDisplaySettings),
                MenuItem::separator(),
                MenuItem::action("Open Output Folder", OpenOutputDir),
            ],
        },
    ]
}

fn main() {
    let app = Application::new();

    // Handle files opened via Finder (double-click on .mp3cd files)
    app.on_open_urls(|urls| {
        for url in urls {
            // URLs are file:// URLs, convert to path
            if let Some(path_str) = url.strip_prefix("file://") {
                // URL decode the path (spaces become %20, etc.)
                let decoded = percent_decode_str(path_str);
                let path = std::path::PathBuf::from(&decoded);
                if path.extension().is_some_and(|ext| ext == "mp3cd") {
                    println!("File opened from Finder: {:?}", path);
                    push_pending_file(path);
                }
            }
        }
    });

    app.run(|cx: &mut App| {
        // Load app settings from disk (or use defaults)
        cx.set_global(AppSettings::load());
        // Load display settings from disk (or use defaults)
        cx.set_global(DisplaySettings::load());
        // Load window state from disk (for position/size)
        let window_state = WindowState::load();

        // Register action handlers (Quit is registered later, after conversion state global is set)
        cx.on_action(|_: &About, cx| {
            AboutBox::open(cx);
        });
        cx.on_action(|_: &OpenOutputDir, _cx| {
            let output_dir = conversion::get_output_dir();
            if output_dir.exists() {
                let _ = std::process::Command::new("open").arg(&output_dir).spawn();
            } else {
                println!("Output directory does not exist yet: {:?}", output_dir);
            }
        });
        cx.on_action(|_: &ToggleSimulateBurn, cx| {
            // Toggle the setting
            let settings = cx.global_mut::<AppSettings>();
            settings.simulate_burn = !settings.simulate_burn;
            println!("Simulate burn: {}", settings.simulate_burn);

            // Rebuild menus to show updated checkmark
            let menus = build_menus(settings);
            cx.set_menus(menus);

            // Save settings to disk
            if let Err(e) = cx.global::<AppSettings>().save() {
                eprintln!("Failed to save settings: {}", e);
            }
        });
        // Note: ToggleEmbedAlbumArt handler is registered after window creation
        // so it can access the window_handle to notify the encoder.
        cx.on_action(|_: &OpenDisplaySettings, cx| {
            DisplaySettingsModal::open(cx);
        });

        // Note: Profile action handlers are registered on the FolderList view itself
        // via on_action in render(). The view has focus, so it receives the actions
        // dispatched from menu items.

        // Bind keyboard shortcuts
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewProfile, None),
            KeyBinding::new("cmd-o", OpenProfile, None),
            KeyBinding::new("cmd-s", SaveProfile, None),
        ]);

        // Set up the initial application menu
        let settings = cx.global::<AppSettings>();
        cx.set_menus(build_menus(settings));

        // Open the main window with saved position/size
        let bounds = Bounds::new(
            point(px(window_state.x as f32), px(window_state.y as f32)),
            size(
                px(window_state.width as f32),
                px(window_state.height as f32),
            ),
        );

        // Use shared cells to pass handles out of the window creation closure
        let encoder_handle_cell: std::sync::Arc<
            std::sync::Mutex<Option<conversion::BackgroundEncoderHandle>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(None));
        let encoder_handle_for_closure = encoder_handle_cell.clone();

        let conversion_state_cell: std::sync::Arc<std::sync::Mutex<Option<core::ConversionState>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let conversion_state_for_closure = conversion_state_cell.clone();

        let window_handle: WindowHandle<FolderList> = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(500.), px(300.))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("MP3 CD Burner".into()),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    ..Default::default()
                },
                |_window, cx| {
                    cx.new(|cx| {
                        let mut folder_list = FolderList::new(cx);

                        // Store conversion state for quit/close guards
                        *conversion_state_for_closure.lock().unwrap() =
                            Some(folder_list.conversion_state.clone());

                        // Enable background encoding for immediate folder conversion
                        match folder_list.enable_background_encoding() {
                            Ok(handle) => {
                                // Store the handle so we can set it as a global
                                *encoder_handle_for_closure.lock().unwrap() = Some(handle);
                                // Start polling for encoder events
                                folder_list.start_encoder_polling(cx);
                            }
                            Err(e) => {
                                eprintln!("Warning: Could not enable background encoding: {}", e);
                                eprintln!("Falling back to legacy mode (convert on burn)");
                            }
                        }
                        folder_list
                    })
                },
            )
            .unwrap();

        // Set the encoder handle as a global for access from action handlers
        if let Some(handle) = encoder_handle_cell.lock().unwrap().take() {
            cx.set_global(handle);
        }

        // Set the conversion state as a global for quit/close guards
        if let Some(state) = conversion_state_cell.lock().unwrap().take() {
            cx.set_global(state);
        }

        // Register Quit handler (after conversion state global is available)
        cx.on_action(|_: &Quit, cx| {
            // Check if a burn is in progress
            if let Some(state) = cx.try_global::<core::ConversionState>()
                && state.is_converting() {
                    // Show warning - don't quit
                    eprintln!("Cannot quit: burn in progress");
                    // TODO: Show a dialog instead of just logging
                    return;
                }
            cx.quit();
        });

        // Register ToggleEmbedAlbumArt handler
        cx.on_action(|_: &ToggleEmbedAlbumArt, cx| {
            // Toggle the setting
            let settings = cx.global_mut::<AppSettings>();
            settings.embed_album_art = !settings.embed_album_art;
            let embed = settings.embed_album_art;
            println!("[main.rs] Toggled embed_album_art = {}", embed);

            // Rebuild menus to show updated checkmark
            let menus = build_menus(settings);
            cx.set_menus(menus);

            // Notify the encoder via the global handle
            if let Some(encoder) = cx.try_global::<conversion::BackgroundEncoderHandle>() {
                encoder.set_embed_album_art(embed);
                println!("[main.rs] Notified encoder");
            } else {
                println!("[main.rs] No encoder global available");
            }

            // Save settings to disk
            if let Err(e) = cx.global::<AppSettings>().save() {
                eprintln!("Failed to save settings: {}", e);
            }
        });

        // Quit the app when the main window is closed (not other windows like dialogs)
        // Window state is saved via observe_window_bounds in FolderList
        let main_window_id = window_handle.window_id();
        cx.on_window_closed(move |cx| {
            // Only quit if the main window was closed, not dialogs
            // Check if main window still exists in the list of open windows
            let main_window_open = cx.windows().iter().any(|w| w.window_id() == main_window_id);
            if !main_window_open {
                // Don't quit if a burn is in progress - show progress window instead
                if let Some(state) = cx.try_global::<core::ConversionState>()
                    && state.is_converting() {
                        println!("Window closed during burn - opening progress window");
                        ui::components::BurnProgressWindow::open(cx, state.clone());
                        return;
                    }
                cx.quit();
            }
        })
        .detach();

        cx.activate(true);
    });
}
