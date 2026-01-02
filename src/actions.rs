//! Application-wide actions
//!
//! Actions that can be triggered from menus or keyboard shortcuts.

use gpui::actions;

// Define actions for menu items
actions!(app, [
    Quit,
    About,
    OpenOutputDir,
    ToggleSimulateBurn,
    // Profile actions
    NewProfile,
    OpenProfile,
    SaveProfile,
]);
