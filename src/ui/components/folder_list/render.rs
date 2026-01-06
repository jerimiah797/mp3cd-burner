//! Rendering implementation for FolderList
//!
//! Contains the Render trait implementation and all rendering helper methods.

use std::sync::atomic::Ordering;

use gpui::{
    Context, ExternalPaths, IntoElement, Render, SharedString, Window, div, prelude::*, rgb,
};

use crate::actions::{NewProfile, OpenProfile, SaveProfile, SetVolumeLabel};
use crate::core::{BurnStage, DisplaySettings, FolderConversionStatus, WindowState};
use crate::ui::Theme;

use gpui::PromptLevel;

use super::{FolderList, PendingBurnAction};
use crate::ui::components::folder_item::{DraggedFolder, FolderItemProps, render_folder_item};
use crate::ui::components::status_bar::{
    StatusBarState, is_stage_cancelable, render_burn_button_base, render_clickable_bitrate,
    render_convert_burn_button_base, render_erase_burn_button_base, render_import_progress,
    render_iso_too_large, render_progress_box, render_stats_panel,
};

impl FolderList {
    /// Render the empty state drop zone
    pub(super) fn render_empty_state(&self, theme: &Theme) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(theme.text_muted)
            .child(div().text_2xl().child("📂"))
            .child(div().text_lg().child("Drop music folders here"))
            .child(div().text_sm().child("or drag items to reorder"))
    }

    /// Render the populated folder list
    pub(super) fn render_folder_items(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let drop_target = self.drop_target_index;
        // Clone display settings to avoid borrow conflict with cx
        let display_settings = cx.global::<DisplaySettings>().clone();
        let mut list = div().w_full().flex().flex_col().gap_2();

        for (index, folder) in self.folders.iter().enumerate() {
            // Get live conversion status from encoder state (for progress updates)
            let live_status = self.get_folder_conversion_status(&folder.id);
            let mut folder_with_live_status = folder.clone();
            // Only update if actively converting (preserve Converted status from folder)
            if matches!(live_status, FolderConversionStatus::Converting { .. }) {
                folder_with_live_status.conversion_status = live_status;
            }

            let props = FolderItemProps {
                index,
                folder: folder_with_live_status,
                is_drop_target: drop_target == Some(index),
                theme: *theme,
                show_file_count: display_settings.show_file_count,
                show_original_size: display_settings.show_original_size,
                show_converted_size: display_settings.show_converted_size,
                show_source_format: display_settings.show_source_format,
                show_source_bitrate: display_settings.show_source_bitrate,
                show_final_bitrate: display_settings.show_final_bitrate,
            };

            let item = render_folder_item(
                props,
                cx,
                |view: &mut Self, from, to| {
                    view.move_folder(from, to);
                    view.drop_target_index = None;
                },
                |view: &mut Self, idx| {
                    view.remove_folder(idx);
                },
            );

            list = list.child(item);
        }

        list
    }

    /// Build the StatusBarState from current FolderList state
    pub(super) fn build_status_bar_state(&self) -> StatusBarState {
        StatusBarState {
            total_files: self.total_files(),
            total_size: self.total_size(),
            total_duration: self.total_duration(),
            bitrate_estimate: self.calculated_bitrate_estimate(),
            has_folders: !self.folders.is_empty(),
            is_importing: self.import_state.is_importing(),
            import_progress: self.import_state.progress(),
            is_converting: self.conversion_state.is_converting(),
            conversion_progress: self.conversion_state.progress(),
            burn_stage: self.conversion_state.get_stage(),
            burn_progress: self.conversion_state.get_burn_progress(),
            is_cancelled: self.conversion_state.is_cancelled(),
            can_burn_another: self.can_burn_another(),
            iso_exceeds_limit: self.iso_exceeds_limit(),
            iso_size_mb: self.iso_size_mb(),
            iso_has_been_burned: self.iso_has_been_burned,
            is_manual_override: self.manual_bitrate_override.is_some(),
            effective_bitrate: self.calculated_bitrate(), // Respects manual override
            is_bitrate_preliminary: self.is_bitrate_preliminary(),
        }
    }

    /// Render the status bar with detailed stats and action button
    pub(super) fn render_status_bar(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.build_status_bar_state();
        let success_color = theme.success;
        let success_hover = theme.success_hover;
        let text_muted = theme.text_muted;
        let text_color = theme.text;
        let is_clickable = state.should_show_bitrate();

        // Build clickable bitrate element
        let mut bitrate_el = render_clickable_bitrate(&state, theme);
        if is_clickable {
            bitrate_el = bitrate_el.on_click(cx.listener(|this, _event, _window, cx| {
                this.show_bitrate_override_dialog(cx);
            }));
        }

        // Build row 3: Bitrate, ISO, CD-RW
        let bitrate_row = div()
            .flex()
            .gap_4()
            .text_color(text_muted)
            .text_sm()
            .child(bitrate_el)
            // ISO size (only show when we have a valid ISO)
            .when(state.iso_size_mb.is_some(), |el| {
                let iso_mb = state.iso_size_mb.unwrap_or(0.0);
                el.child(
                    div().flex().gap_1().child("ISO:").child(
                        div()
                            .text_color(text_color)
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(format!("{:.0} MB", iso_mb)),
                    ),
                )
            })
            // CD-RW indicator (only show when erasable disc detected)
            .when(
                state.is_converting && state.burn_stage == BurnStage::ErasableDiscDetected,
                |el| {
                    el.child(
                        div()
                            .text_color(theme.danger)
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("CD-RW"),
                    )
                },
            );

        // Build left side: stats panel (rows 1-2) + bitrate row
        let left_panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(render_stats_panel(&state, theme))
            .child(bitrate_row);

        div()
            .py_4()
            .px_6()
            .flex()
            .items_center()
            .justify_between()
            .bg(theme.bg)
            .border_t_1()
            .border_color(theme.border)
            .text_sm()
            // Left side: stats panel with clickable bitrate
            .child(left_panel)
            // Right side: action panel
            .child(self.render_action_panel(
                &state,
                theme,
                success_color,
                success_hover,
                text_muted,
                cx,
            ))
    }

    /// Render the right action panel (progress displays and buttons)
    fn render_action_panel(
        &self,
        state: &StatusBarState,
        theme: &Theme,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        text_muted: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if state.is_importing {
            render_import_progress(state, theme).into_any_element()
        } else if state.is_converting {
            self.render_conversion_progress(state, theme, success_color, success_hover, cx)
                .into_any_element()
        } else if state.can_burn_another && state.iso_exceeds_limit {
            render_iso_too_large(state.iso_size_mb.unwrap_or(0.0), theme).into_any_element()
        } else if state.can_burn_another {
            self.render_burn_button(state.iso_has_been_burned, success_color, success_hover, cx)
                .into_any_element()
        } else {
            self.render_convert_burn_button(
                state.has_folders,
                success_color,
                success_hover,
                text_muted,
                cx,
            )
            .into_any_element()
        }
    }

    /// Render conversion/burn progress with cancel support
    fn render_conversion_progress(
        &self,
        state: &StatusBarState,
        theme: &Theme,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_cancelable = is_stage_cancelable(state);

        div()
            .id(SharedString::from("convert-progress-container"))
            .flex()
            .flex_col()
            .gap_2()
            .items_center()
            // Progress display (hide when waiting for user to approve erase)
            .when(state.burn_stage != BurnStage::ErasableDiscDetected, |el| {
                let mut progress_box = render_progress_box(state, theme);
                if is_cancelable {
                    progress_box = progress_box.cursor_pointer().on_click(cx.listener(
                        |this, _event, _window, _cx| {
                            this.conversion_state.request_cancel();
                        },
                    ));
                }
                el.child(progress_box)
            })
            // Erase & Burn button (only show when erasable disc detected)
            .when(state.burn_stage == BurnStage::ErasableDiscDetected, |el| {
                el.child(
                    render_erase_burn_button_base(success_color, success_hover).on_click(
                        cx.listener(|this, _event, _window, _cx| {
                            println!("Erase & Burn clicked");
                            this.conversion_state
                                .erase_approved
                                .store(true, Ordering::SeqCst);
                        }),
                    ),
                )
            })
    }

    /// Render Burn/Burn Another button
    fn render_burn_button(
        &self,
        iso_has_been_burned: bool,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        render_burn_button_base(iso_has_been_burned, success_color, success_hover).on_click(
            cx.listener(move |this, _event, _window, cx| {
                println!("Burn clicked - showing volume label dialog");
                this.show_volume_label_dialog(Some(PendingBurnAction::BurnExisting), cx);
            }),
        )
    }

    /// Render Convert & Burn button
    fn render_convert_burn_button(
        &self,
        has_folders: bool,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        text_muted: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        render_convert_burn_button_base(has_folders, success_color, success_hover, text_muted)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if has_folders {
                    println!("Convert & Burn clicked - showing volume label dialog");
                    this.show_volume_label_dialog(Some(PendingBurnAction::ConvertAndBurn), cx);
                }
            }))
    }

    /// Show any pending error dialog
    ///
    /// This is called from the render loop to display error messages
    /// like failed folder loads.
    pub(super) fn show_pending_error_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((title, message)) = self.pending_error_message.take() {
            let _future = window.prompt(
                PromptLevel::Warning,
                &title,
                Some(&message),
                &["OK"],
                cx,
            );
            // We don't need to wait for the response - just showing the dialog
        }
    }

    pub(super) fn show_pending_info_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((title, message)) = self.pending_info_message.take() {
            let _future = window.prompt(
                PromptLevel::Info,
                &title,
                Some(&message),
                &["OK"],
                cx,
            );
            // We don't need to wait for the response - just showing the dialog
        }
    }
}

impl Render for FolderList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to appearance changes and register action handlers (once)
        if !self.appearance_subscription_set {
            self.appearance_subscription_set = true;
            cx.observe_window_appearance(window, |_this, _window, cx| {
                cx.notify();
            })
            .detach();
        }

        // Subscribe to bounds changes to save window state (once)
        if !self.bounds_subscription_set {
            self.bounds_subscription_set = true;
            cx.observe_window_bounds(window, |_this, window, _cx| {
                let bounds = window.bounds();
                let state = WindowState {
                    x: bounds.origin.x.into(),
                    y: bounds.origin.y.into(),
                    width: bounds.size.width.into(),
                    height: bounds.size.height.into(),
                };
                if let Err(e) = state.save() {
                    eprintln!("Failed to save window state: {}", e);
                }
            })
            .detach();
        }

        // Grab initial focus so menu items work immediately
        if self.needs_initial_focus {
            self.needs_initial_focus = false;
            if let Some(ref focus_handle) = self.focus_handle {
                focus_handle.focus(window);
            }
        }

        // Show any pending dialogs
        self.show_pending_error_dialog(window, cx);
        self.show_pending_info_dialog(window, cx);

        // Check for files opened via Finder (double-click on .mp3cd files)
        self.poll_pending_open_files(cx);

        // Check for pending burn action after volume label dialog closes
        self.check_pending_burn_action(window, cx);

        // Show pending error messages (e.g., failed folder loads)
        self.show_pending_error_dialog(window, cx);

        // Update window title to include volume label
        let title = if self.volume_label == "Untitled MP3CD" || self.volume_label.is_empty() {
            "MP3 CD Burner".to_string()
        } else {
            format!("MP3 CD Burner - {}", self.volume_label)
        };
        window.set_window_title(&title);

        // Get theme based on OS appearance
        let theme = Theme::from_appearance(window.appearance());
        let is_empty = self.folders.is_empty();

        // Build the folder list content
        let list_content = if is_empty {
            self.render_empty_state(&theme).into_any_element()
        } else {
            self.render_folder_items(&theme, cx).into_any_element()
        };

        // Capture all listeners first (before borrowing for status bar)
        let on_external_drop = cx.listener(|this, paths: &ExternalPaths, window, cx| {
            // Check if any dropped path is a .mp3cd profile file
            let profile_path = paths.paths().iter().find(|p| {
                p.extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase() == "mp3cd")
                    .unwrap_or(false)
            });

            if let Some(profile) = profile_path {
                // Load as profile instead of treating as music folder
                // This handles unsaved changes check like File > Open
                println!("Loading dropped profile: {:?}", profile);
                this.load_dropped_profile(profile.clone(), window, cx);
            } else {
                // No profile files - treat as music folders
                this.add_external_folders(paths.paths(), cx);
            }
            this.drop_target_index = None;
        });

        let on_internal_drop = cx.listener(|this, dragged: &DraggedFolder, _window, _cx| {
            let target = this.folders.len();
            this.move_folder(dragged.index, target);
            this.drop_target_index = None;
        });

        // Profile action handlers
        let on_new_profile = cx.listener(|this, _: &NewProfile, window, cx| {
            this.new_profile(window, cx);
        });
        let on_open_profile = cx.listener(|this, _: &OpenProfile, window, cx| {
            this.open_profile(window, cx);
        });
        let on_save_profile = cx.listener(|this, _: &SaveProfile, window, cx| {
            this.save_profile_dialog(window, cx);
        });
        let on_set_volume_label = cx.listener(|this, _: &SetVolumeLabel, _window, cx| {
            this.show_volume_label_dialog(None, cx);
        });

        // Build status bar after listeners
        let status_bar = self.render_status_bar(&theme, cx);

        // Build the base container
        let mut container = div().size_full().flex().flex_col().bg(theme.bg);

        // Track focus if we have a focus handle (not in tests)
        if let Some(ref focus_handle) = self.focus_handle {
            container = container.track_focus(focus_handle);
        }

        container
            .on_action(on_new_profile)
            .on_action(on_open_profile)
            .on_action(on_save_profile)
            .on_action(on_set_volume_label)
            // Handle external file drops on the entire window
            .on_drop(on_external_drop)
            // Style when dragging external files over window
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
            // Main content area - folder list (scrollable)
            .child(
                div()
                    .id("folder-list-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll_handle)
                    .px_6() // Horizontal padding for breathing room
                    .py_2() // Vertical padding
                    // Handle drops on the list container
                    .on_drop(on_internal_drop)
                    .drag_over::<DraggedFolder>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
                    .child(list_content),
            )
            // Status bar at bottom
            .child(status_bar)
    }
}
