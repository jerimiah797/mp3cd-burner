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
use actions::{Quit, About, OpenOutputDir, ToggleSimulateBurn, NewProfile, OpenProfile, SaveProfile};
use core::AppSettings;
use ui::components::{AboutBox, FolderList};

/// Build the application menus with current settings state
fn build_menus(settings: &AppSettings) -> Vec<Menu> {
    // Use checkmark prefix when enabled
    let simulate_burn_label = if settings.simulate_burn {
        "✓ Simulate Burn"
    } else {
        "Simulate Burn"
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
                MenuItem::action("Embed Album Art", About), // TODO: Implement toggle
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
                    if let Err(e) = folder_list.enable_background_encoding() {
                        eprintln!("Warning: Could not enable background encoding: {}", e);
                        eprintln!("Falling back to legacy mode (convert on burn)");
                    } else {
                        // Start polling for encoder events
                        folder_list.start_encoder_polling(cx);
                    }
                    folder_list
                })
            },
        )
        .unwrap();

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
