//! MP3 CD Burner - GPUI Application
//!
//! A native macOS application for converting music folders to MP3
//! and burning them to CD.

mod audio;
mod burning;
mod conversion;
mod core;
mod profiles;
mod ui;

use gpui::{
    actions, prelude::*, px, size, App, Application, Bounds, KeyBinding, Menu, MenuItem,
    WindowBounds, WindowOptions,
};
use ui::components::FolderList;

// Define actions for menu items
actions!(app, [Quit, About]);

fn main() {
    Application::new().run(|cx: &mut App| {
        // Register action handlers
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &About, _cx| {
            println!("MP3 CD Burner v0.1.0 - Built with GPUI");
        });

        // Bind keyboard shortcuts
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

        // Set up the application menu
        cx.set_menus(vec![
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
                    MenuItem::action("Simulate Burn", About), // TODO: Implement toggle
                    MenuItem::action("No Lossy Conversions", About), // TODO: Implement toggle
                    MenuItem::action("Embed Album Art", About), // TODO: Implement toggle
                ],
            },
        ]);

        // Open the main window
        let bounds = Bounds::centered(None, size(px(500.), px(600.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("MP3 CD Burner".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| FolderList::new()),
        )
        .unwrap();

        cx.activate(true);
    });
}
