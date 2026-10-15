//! The window's title bar on macOS: transparent, with the content under it, and an empty
//! "unified compact" toolbar, which makes the bar 40 pt tall and centres the traffic lights in
//! it (macOS's own layout; nothing is moved by hand). The bar's height and where the traffic
//! lights end are read back from AppKit, so the app's top bar lines up on any macOS version.
//!
//! Measured on macOS 27 with an off-screen probe window: no toolbar 32 pt (lights centred 16 pt
//! from the top, x 9–69 pt); unified compact 40 pt (20 pt, x 12–72 pt); unified 52 pt. Clicks in
//! the bar reach the content view in all three, so egui receives them.

/// The title bar the app's top bar covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitleBar {
    /// Height in points; the traffic lights are centred in it.
    pub height: f32,
    /// Right edge of the traffic lights, in points from the window's left edge.
    pub lights_end: f32,
}

impl Default for TitleBar {
    /// The unified compact layout as measured on macOS 27 (also used by the headless tests).
    fn default() -> Self {
        Self { height: 40.0, lights_end: 72.0 }
    }
}

/// Adds the toolbar to the app's window and reads the resulting layout. `None` when the window
/// is not an AppKit window or this is not the main thread.
#[cfg(target_os = "macos")]
pub fn install(window: &impl raw_window_handle::HasWindowHandle) -> Option<TitleBar> {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSTitlebarSeparatorStyle, NSToolbar, NSView, NSWindowButton, NSWindowToolbarStyle};
    use objc2_foundation::NSString;
    use raw_window_handle::RawWindowHandle;

    let mtm = MainThreadMarker::new()?;
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit hands out its live NSView, and we are on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view.window()?;
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str("musicbox.titlebar"));
    #[allow(deprecated)]
    toolbar.setShowsBaselineSeparator(false);
    window.setToolbar(Some(&toolbar));
    window.setToolbarStyle(NSWindowToolbarStyle::UnifiedCompact);
    window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
    let height = window.frame().size.height - window.contentLayoutRect().size.height;
    let zoom = window.standardWindowButton(NSWindowButton::ZoomButton)?;
    let lights = zoom.convertRect_toView(zoom.bounds(), None);
    let bar = TitleBar { height: height as f32, lights_end: (lights.origin.x + lights.size.width) as f32 };
    // A window in full screen or without a title bar reports nothing useful.
    (bar.height >= 20.0 && bar.height <= 80.0).then_some(bar)
}
