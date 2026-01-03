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

use gpui::{
    prelude::*, px, size, App, Application, Bounds, KeyBinding, Menu, MenuItem,
    WindowBounds, WindowHandle, WindowOptions,
};
use actions::{Quit, About, OpenOutputDir, ToggleSimulateBurn, ToggleEmbedAlbumArt, OpenDisplaySettings, NewProfile, OpenProfile, SaveProfile};
use core::{AppSettings, DisplaySettings};
use ui::components::{AboutBox, DisplaySettingsModal, FolderList};

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
                MenuItem::action("No Lossy Conversions", About), // TODO: Implement toggle
                MenuItem::action(embed_album_art_label, ToggleEmbedAlbumArt),
                MenuItem::separator(),
                MenuItem::action("Display Settings...", OpenDisplaySettings),
                MenuItem::separator(),
                MenuItem::action("Open Output Folder", OpenOutputDir),
            ],
        },
    ]
}

fn main() {
    Application::new().run(|cx: &mut App| {
        // Initialize global app settings
        cx.set_global(AppSettings::default());
        // Load display settings from disk (or use defaults)
        cx.set_global(DisplaySettings::load());

        // Register action handlers
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &About, cx| {
            AboutBox::open(cx);
        });
        cx.on_action(|_: &OpenOutputDir, _cx| {
            let output_dir = conversion::get_output_dir();
            if output_dir.exists() {
                let _ = std::process::Command::new("open")
                    .arg(&output_dir)
                    .spawn();
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

        // Open the main window
        let bounds = Bounds::centered(None, size(px(500.), px(600.)), cx);

        // Use a shared cell to pass the encoder handle out of the window creation closure
        let encoder_handle_cell: std::sync::Arc<std::sync::Mutex<Option<conversion::BackgroundEncoderHandle>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let encoder_handle_for_closure = encoder_handle_cell.clone();

        let window_handle: WindowHandle<FolderList> = cx.open_window(
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
        });

        // Quit the app when the main window is closed
        // This is appropriate for a single-window utility app
        cx.on_window_closed(|cx| {
            cx.quit();
        })
        .detach();

        // Suppress unused warning - window_handle keeps the window alive
        let _ = window_handle;

        cx.activate(true);
    });
}
