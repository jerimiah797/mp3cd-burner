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
    WindowBounds, WindowOptions,
};
use actions::{Quit, About, OpenOutputDir, ToggleSimulateBurn};
use core::AppSettings;
use ui::components::FolderList;

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
                MenuItem::action("New", About), // TODO: Implement New action
                MenuItem::action("Open Profile...", About), // TODO: Implement
                MenuItem::separator(),
                MenuItem::action("Save Profile", About), // TODO: Implement
                MenuItem::action("Save Profile As...", About), // TODO: Implement
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
        cx.on_action(|_: &About, _cx| {
            println!("MP3 CD Burner v0.1.0 - Built with GPUI");
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

        // Bind keyboard shortcuts
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

        // Set up the initial application menu
        let settings = cx.global::<AppSettings>();
        cx.set_menus(build_menus(settings));

        // Open the main window
        let bounds = Bounds::centered(None, size(px(500.), px(600.)), cx);

        cx.open_window(
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
            |_window, cx| cx.new(|cx| FolderList::new(cx)),
        )
        .unwrap();

        cx.activate(true);
    });
}
