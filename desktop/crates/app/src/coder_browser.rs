//! The preview pane — a real browser, parented into the app's own window.
//!
//! iced draws with wgpu and has no web engine, so the page cannot be a widget.
//! What it can be is a **child window**: `wry` builds a WebView2 surface as a
//! child of our `HWND` and paints it over the region the layout leaves empty
//! ([`WIDTH`] logical pixels down the right-hand edge). The iced side reserves
//! exactly that strip and draws the URL bar in it; everything below the bar is
//! the child's.
//!
//! Two consequences fall out of "child window", and both are deliberate:
//!
//! - **The webview is not `Send`.** It lives in a `thread_local` on the winit
//!   thread — the same thread `update` and [`iced::window::run`] run on — rather
//!   than in `State`, which crosses threads.
//! - **A child window has no z-order relative to wgpu content.** It sits over
//!   whatever iced draws in that strip, including a modal's scrim. So it is
//!   hidden, not merely un-drawn, whenever the pane is closed or the screen is
//!   not Coder — see [`Cmd::Hide`] and its callers in `main::update`.
//!
//! `ponytail:` the pane is a fixed width rather than a draggable split, and a
//! modal over it hides it wholesale. A splitter needs the child rect to track a
//! widget's layout, which iced does not report back to `update`; that is the
//! upgrade path if the width ever needs to move.

use iced::{window, Task};

/// Logical width of the preview strip. The iced layout reserves exactly this,
/// so the two must not drift — both read this constant.
pub const WIDTH: f32 = 520.0;

/// Logical height of the URL bar iced draws above the child window.
pub const BAR_HEIGHT: f32 = 44.0;

/// What the one-click button in the empty pane opens. A dev server on 3000 is
/// the common case (Next, CRA, most `npm run dev`); anything else is one edit
/// of the URL bar away.
pub const DEFAULT_URL: &str = "http://localhost:3000";

/// What the pane should do. Kept coarse: every arm ends with the child window
/// in a known position, so a resize and a navigation are the same code path.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Create if needed, position, show. Also the resize/re-show path.
    Show,
    /// Keep the page and its session alive, take it off the screen.
    Hide,
    Load(String),
    Back,
    Forward,
    Reload,
}

/// Run `cmd` against the preview, resolving the window and its size first.
///
/// The size comes from iced rather than from `GetClientRect` so the numbers are
/// logical pixels in the same space as the layout above — no DPI arithmetic of
/// our own to get wrong.
pub fn run<M: Send + 'static>(
    cmd: Cmd,
    on_done: impl Fn(Result<(), String>) -> M + Send + Clone + 'static,
) -> Task<M> {
    window::latest().and_then(move |id| {
        let cmd = cmd.clone();
        let on_done = on_done.clone();
        window::size(id).then(move |size| {
            let cmd = cmd.clone();
            window::run(id, move |handle| apply(handle, size, cmd)).map(on_done.clone())
        })
    })
}

#[cfg(windows)]
mod imp {
    use super::{Cmd, BAR_HEIGHT, WIDTH};
    use iced::Size;
    use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CLIPCHILDREN,
    };
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{Rect, WebView, WebViewBuilder};

    thread_local! {
        /// The one preview. Not in `State`: `WebView` is `!Send`, and this is
        /// the thread it was built on and the only thread that touches it.
        static VIEW: RefCell<Option<WebView>> = const { RefCell::new(None) };
    }

    /// `build_as_child` wants a sized `HasWindowHandle`; iced hands us a
    /// `&dyn Window`. The handle is borrowed for the call only, and the parent
    /// window outlives it by construction — it is the window we are drawing in.
    struct Parent(RawWindowHandle);

    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            Ok(unsafe { WindowHandle::borrow_raw(self.0.clone()) })
        }
    }

    /// Add `WS_CLIPCHILDREN` to the app window.
    ///
    /// Without it the parent paints its whole client area, and iced's DXGI
    /// swapchain present goes straight over the child window every frame: the
    /// webview is created, visible, correctly positioned — and invisible.
    /// Measured exactly that way before this call existed. `WS_CLIPCHILDREN`
    /// excludes child regions from the parent's painting, which is what makes
    /// the two surfaces coexist.
    fn clip_children(raw: RawWindowHandle) {
        let RawWindowHandle::Win32(w) = raw else { return };
        let hwnd = w.hwnd.get() as isize as HWND;
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            if style & WS_CLIPCHILDREN as isize == 0 {
                SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
                // The style cache is per-window and only re-read on a frame
                // change, so say so or the next present ignores it.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
        }
    }

    fn bounds(size: Size) -> Rect {
        Rect {
            position: LogicalPosition::new(
                (size.width - WIDTH).max(0.0) as f64,
                BAR_HEIGHT as f64,
            )
            .into(),
            size: LogicalSize::new(
                WIDTH.min(size.width) as f64,
                (size.height - BAR_HEIGHT).max(0.0) as f64,
            )
            .into(),
        }
    }

    pub fn apply(
        handle: &dyn iced::window::Window,
        size: Size,
        cmd: Cmd,
    ) -> Result<(), String> {
        VIEW.with(|slot| {
            let mut slot = slot.borrow_mut();
            if matches!(cmd, Cmd::Hide) {
                // Nothing built yet is already hidden.
                return match slot.as_ref() {
                    Some(v) => v.set_visible(false).map_err(|e| e.to_string()),
                    None => Ok(()),
                };
            }
            if slot.is_none() {
                let raw = handle
                    .window_handle()
                    .map_err(|e| format!("no window handle: {e}"))?
                    .as_raw();
                let start = match &cmd {
                    Cmd::Load(url) => url.clone(),
                    _ => "about:blank".to_string(),
                };
                let view = WebViewBuilder::new()
                    .with_url(&start)
                    .with_bounds(bounds(size))
                    .build_as_child(&Parent(raw))
                    .map_err(|e| format!("could not start the preview: {e}"))?;
                clip_children(raw);
                *slot = Some(view);
            }
            let view = slot.as_ref().expect("just built");
            view.set_bounds(bounds(size)).map_err(|e| e.to_string())?;
            match &cmd {
                Cmd::Load(url) => view.load_url(url).map_err(|e| e.to_string())?,
                // History and reload are the page's own, so they are the page's
                // API rather than a second navigation stack of ours.
                Cmd::Back => view.evaluate_script("history.back()").map_err(|e| e.to_string())?,
                Cmd::Forward => {
                    view.evaluate_script("history.forward()").map_err(|e| e.to_string())?
                }
                Cmd::Reload => {
                    view.evaluate_script("location.reload()").map_err(|e| e.to_string())?
                }
                Cmd::Show | Cmd::Hide => {}
            }
            view.set_visible(true).map_err(|e| e.to_string())
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Cmd;
    use iced::Size;

    /// Everywhere else the pane draws its empty strip and says so. The child-
    /// window trick is per-platform, and this app ships on Windows.
    pub fn apply(_: &dyn iced::window::Window, _: Size, _: Cmd) -> Result<(), String> {
        Err("The preview pane is only available on Windows in this build.".into())
    }
}

use imp::apply;
