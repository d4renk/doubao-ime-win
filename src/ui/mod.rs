//! UI Module
//!
//! Handles system tray and floating button UI.

mod floating_button;
mod settings_window;
mod system_tray;

pub use floating_button::{
    ButtonState, FloatingButton, FloatingButtonConfig, FloatingButtonEvent,
    FloatingButtonStateSetter,
};
pub use settings_window::show_settings;
pub use system_tray::run_app;
