//! The window's title bar on macOS: transparent, with the content under it, and an empty
//! "unified compact" toolbar, which makes the bar 40 pt tall and centres the traffic lights in
//! it (macOS's own layout; nothing is moved by hand). The bar's height and where the traffic
//! lights end are read back from AppKit, so the app's top bar lines up on any macOS version.
//!
//! Measured on macOS 27 with an off-screen probe window: no toolbar 32 pt (lights centred 16 pt
//! from the top, x 9–69 pt); unified compact 40 pt (20 pt, x 12–72 pt); unified 52 pt. Clicks in
//! the bar reach the content view in all three, so egui receives them.
//!
//! Dragging the top bar moves the window through a native event monitor ([`install_drag`]).

/// The part of the top bar that moves the window, in points from the content's top-left corner
/// (the window's, as the content runs under the title bar). The app sets it every frame.
pub type DragArea = std::sync::Arc<std::sync::Mutex<Option<egui::Rect>>>;

/// Moves the window from the top bar ([`install_drag`]); dropping it ends that.
pub struct WindowDrag {
    pub area: DragArea,
    #[cfg(target_os = "macos")]
    _monitor: objc2::rc::Retained<objc2::runtime::AnyObject>,
}

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

/// Starts window drags from the top bar's empty part at the mouse-down itself, as AppKit expects:
/// `performWindowDragWithEvent:` takes the original mouse-down and hands the drag to the Window
/// Server, which then moves the window on its own. A local event monitor sees the mouse-down
/// before the content view does and swallows it when it lands in the [`DragArea`].
///
/// Going through egui instead (`ViewportCommand::StartDrag`) calls the same method from a later
/// frame with whatever event AppKit is handling then. While the preview plays and frames run
/// continuously, that started drags late, and the window jumped to the pointer.
///
/// A double click zooms or minimises, following the system setting for double-clicking a title
/// bar. Nothing happens in full screen.
#[cfg(target_os = "macos")]
pub fn install_drag(window: &impl raw_window_handle::HasWindowHandle) -> Option<WindowDrag> {
    use objc2::MainThreadMarker;
    use objc2::rc::Weak;
    use objc2_app_kit::{NSEvent, NSEventMask, NSView, NSWindowStyleMask};
    use raw_window_handle::RawWindowHandle;
    use std::ptr::NonNull;

    let mtm = MainThreadMarker::new()?;
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit hands out its live NSView, and we are on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let target = Weak::from_retained(&view.window()?);
    let area = DragArea::default();
    let shared = area.clone();
    let handler = block2::RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a live event to the monitor.
        let ev = unsafe { event.as_ref() };
        let rect = shared.lock().ok().and_then(|a| *a);
        if let (Some(window), Some(rect)) = (target.load(), rect)
            && ev.window(mtm).is_some_and(|w| std::ptr::eq(&*w, &*window))
            && !window.styleMask().contains(NSWindowStyleMask::FullScreen)
        {
            let height = window.contentView().map_or(window.frame().size.height, |v| v.frame().size.height);
            let at = ev.locationInWindow();
            if rect.contains(egui::pos2(at.x as f32, (height - at.y) as f32)) {
                if ev.clickCount() >= 2 {
                    title_double_click(&window);
                } else {
                    window.performWindowDragWithEvent(ev);
                }
                return std::ptr::null_mut();
            }
        }
        event.as_ptr()
    });
    // SAFETY: the handler returns the event it was given or null.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::LeftMouseDown, &handler) }?;
    Some(WindowDrag { area, _monitor: monitor })
}

/// What a double click on a title bar does: the system setting `AppleActionOnDoubleClick`
/// ("Maximize", the default, zooms; "Minimize"; "None").
#[cfg(target_os = "macos")]
fn title_double_click(window: &objc2_app_kit::NSWindow) {
    use objc2_foundation::{NSString, NSUserDefaults};
    let action = NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleActionOnDoubleClick"));
    match action.map(|a| a.to_string()).as_deref() {
        Some("Minimize") => window.performMiniaturize(None),
        Some("None") => {}
        _ => window.performZoom(None),
    }
}
