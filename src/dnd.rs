//! Drag and drop with the OS. Dropping *into* the window is delivered by winit
//! (`RawInput::dropped_files`); the copy-or-move rules live in the domain layer and
//! the shell calls in the infrastructure layer — this module only joins them with
//! the eframe window.

pub use quokka_domain::transfer::{can_drop, is_move};
pub use quokka_infra::shell_dnd::{cursor_in_client, drag_out, keys};

/// Native window handle (HWND on Windows), 0 when unavailable.
pub fn window_handle(frame: &eframe::Frame) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().map(|h| h.as_raw()) {
        #[cfg(windows)]
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}
