//! The launcher's one window: frameless, sized to fit the screen, and locked to the app's own page
//! (rows L5 and L10).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::window::Color;

use tauri::{AppHandle, Emitter, Manager, Monitor, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use url::Url;

use crate::platform::dialogs::KeepOpen;

pub const MAIN: &str = "main";

/// The page's ink (styles.css), so nothing flashes before it paints.
const INK: Color = Color(0x01, 0x09, 0x0d, 0xff);

/// Pushed to the page whenever the window is maximised or restored.
pub const WINDOW_STATE_EVENT: &str = "window-state";

/// Logical sizes: the window and its minimum, never larger than the primary screen's usable area
/// (the part the taskbar does not cover). 1366x768 laptops, or 1920x1080 at 150 % scaling, have less
/// than 760 px of height, and the big button at the bottom must stay on screen.
fn fit(work: Option<(f64, f64)>) -> ((f64, f64), (f64, f64)) {
    let (width, height) = work.unwrap_or((f64::INFINITY, f64::INFINITY));
    (
        (width.min(1280.0), height.min(760.0)),
        (width.min(1024.0), height.min(640.0)),
    )
}

fn work_area(monitor: &Monitor) -> (f64, f64) {
    let scale = monitor.scale_factor();
    let size = monitor.work_area().size;
    (f64::from(size.width) / scale, f64::from(size.height) / scale)
}

/// The app's own page, as each webview serves it, or the Vite dev server in development.
pub fn is_app_url(url: &Url, dev: Option<&Url>) -> bool {
    if let Some(dev) = dev {
        return url.origin() == dev.origin();
    }
    match url.scheme() {
        // WebKitGTK
        "tauri" => url.host_str() == Some("localhost"),
        // WebView2
        "http" | "https" => url.host_str() == Some("tauri.localhost"),
        _ => false,
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let ((width, height), (min_width, min_height)) = fit(app.primary_monitor()?.as_ref().map(work_area));
    let dev = if cfg!(dev) {
        app.config().build.dev_url.clone()
    } else {
        None
    };
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Dauntless Revived Launcher")
        .inner_size(width, height)
        .min_inner_size(min_width, min_height)
        .center()
        .decorations(false)
        .visible(false)
        .background_color(INK)
        .on_navigation(move |url| is_app_url(url, dev.as_ref()))
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished {
                let _ = window.show();
            }
        })
        .build()?;

    let handle = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) => {
            let maximized = handle.is_maximized().unwrap_or(false);
            let _ = handle.emit_to(MAIN, WINDOW_STATE_EVENT, maximized);
        }
        WindowEvent::CloseRequested { api, .. } => {
            let guard = handle.state::<CloseGuard>();
            if guard.closing.load(Ordering::SeqCst) {
                return;
            }
            let Some(reason) = guard.keep_open() else { return };
            // Closing would cut the running game's connection: ask first (row L5)
            api.prevent_close();
            let window = handle.clone();
            tauri::async_runtime::spawn(async move {
                let app = window.app_handle().clone();
                let language = app.state::<crate::controller::ControllerHandle>().language();
                if crate::platform::dialogs::confirm_keep_open(&app, language, reason).await {
                    let _ = window.minimize();
                    if reason == KeepOpen::Relay {
                        let guard = app.state::<CloseGuard>();
                        if let Some(kept) = guard.on_kept.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                            kept();
                        }
                    }
                } else {
                    app.state::<CloseGuard>().closing.store(true, Ordering::SeqCst);
                    let _ = window.close();
                }
            });
        }
        _ => {}
    });
    Ok(window)
}

type KeepOpenReason = Box<dyn Fn() -> Option<KeepOpen> + Send + Sync>;

/// What decides whether closing the window needs the player's confirmation: the controller says whether
/// the relay or the offline services carry a running game, and what to do when the player keeps the
/// launcher open (the "keep open" notice in public mode).
#[derive(Default)]
pub struct CloseGuard {
    reason: Mutex<Option<KeepOpenReason>>,
    on_kept: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    closing: AtomicBool,
}

impl CloseGuard {
    pub fn set(
        &self,
        reason: impl Fn() -> Option<KeepOpen> + Send + Sync + 'static,
        on_kept: impl Fn() + Send + Sync + 'static,
    ) {
        *self.reason.lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(reason));
        *self.on_kept.lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(on_kept));
    }

    fn keep_open(&self) -> Option<KeepOpen> {
        self.reason
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(|reason| reason())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn only_the_app_page_may_load() {
        assert!(is_app_url(&url("tauri://localhost/index.html"), None));
        assert!(is_app_url(&url("http://tauri.localhost/index.html#x"), None));
        assert!(is_app_url(&url("https://tauri.localhost/"), None));
        for other in [
            "https://example.com/",
            "http://localhost:5173/",
            "file:///C:/x.html",
            "tauri://evil/",
            "http://tauri.localhost.example.com/",
        ] {
            assert!(!is_app_url(&url(other), None), "{other}");
        }
        let dev = url("http://localhost:5173/");
        assert!(is_app_url(&url("http://localhost:5173/src/index.html"), Some(&dev)));
        assert!(!is_app_url(&url("http://localhost:5174/"), Some(&dev)));
        assert!(!is_app_url(&url("tauri://localhost/"), Some(&dev)));
    }

    #[test]
    fn the_window_fits_small_screens() {
        assert_eq!(fit(Some((1920.0, 1040.0))), ((1280.0, 760.0), (1024.0, 640.0)));
        assert_eq!(fit(Some((1366.0, 728.0))), ((1280.0, 728.0), (1024.0, 640.0)));
        assert_eq!(fit(Some((1280.0, 680.0))), ((1280.0, 680.0), (1024.0, 640.0)));
        assert_eq!(fit(None), ((1280.0, 760.0), (1024.0, 640.0)));
    }
}
