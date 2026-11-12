//! Tray and menu-bar icon. Closing the window keeps the app (and the tunnel)
//! running there, and the tunnel can be started, stopped or opened from its
//! menu without the window.
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::image::Image;
use tauri::{AppHandle, Emitter, Manager, Wry};

struct TrayItems {
    status: MenuItem<Wry>,
    connect: MenuItem<Wry>,
    disconnect: MenuItem<Wry>,
    /// What the menu shows now, so it is only touched when that changes.
    shown: Mutex<String>,
    /// The icon on screen and the step of the connecting animation.
    icon: Mutex<(Look, usize)>,
}

/// The icon follows the tunnel: dark light when off, a yellow light when
/// connected, and arches lighting up one after another while connecting.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    Off,
    On,
    Connecting,
}

fn frame(look: Look, step: usize) -> Option<Image<'static>> {
    let bytes: &[u8] = match (look, step % 3) {
        (Look::Off, _) => include_bytes!("../icons/tray-off.png"),
        (Look::On, _) => include_bytes!("../icons/tray-on.png"),
        (Look::Connecting, 0) => include_bytes!("../icons/tray-connect-1.png"),
        (Look::Connecting, 1) => include_bytes!("../icons/tray-connect-2.png"),
        (Look::Connecting, _) => include_bytes!("../icons/tray-connect-3.png"),
    };
    Image::from_bytes(bytes).ok()
}

/// The frontend shows these as a notice, or switches to a tab.
#[derive(Clone, serde::Serialize)]
struct TrayResult {
    text: String,
    error: bool,
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Отключено", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Открыть Secret Tunnel", true, None::<&str>)?;
    let connect = MenuItem::with_id(app, "connect", "Подключить", true, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, "disconnect", "Отключить", false, None::<&str>)?;
    let speed = MenuItem::with_id(app, "speed", "Проверить скорость", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Настройки…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти из Secret Tunnel", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &connect,
            &disconnect,
            &PredefinedMenuItem::separator(app)?,
            &speed,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id("main").tooltip("Secret Tunnel").menu(&menu);
    if let Some(icon) = frame(Look::Off, 0) {
        builder = builder.icon(icon);
    }
    // Windows opens the window on a left click and the menu on a right one.
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.show_menu_on_left_click(false);
    }
    builder
        .on_menu_event(|app, event| handle(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                if !cfg!(target_os = "macos") {
                    show_window(tray.app_handle());
                }
            }
        })
        .build(app)?;
    app.manage(TrayItems { status, connect, disconnect, shown: Mutex::default(), icon: Mutex::new((Look::Off, 0)) });
    let handle = app.clone();
    std::thread::spawn(move || loop {
        refresh(&handle);
        std::thread::sleep(Duration::from_millis(350));
    });
    Ok(())
}

fn handle(app: &AppHandle, id: &str) {
    match id {
        "open" => show_window(app),
        "connect" | "disconnect" => {
            let app = app.clone();
            let connect = id == "connect";
            std::thread::spawn(move || {
                let state = app.state::<crate::AppState>();
                let result = if connect { crate::start_tunnel(state) } else { crate::stop_tunnel(state) };
                let notice = match result {
                    Ok(text) => TrayResult { text, error: false },
                    Err(text) => TrayResult { text, error: true },
                };
                let _ = app.emit("tray-result", notice);
                refresh(&app);
            });
        }
        "settings" | "speed" => {
            show_window(app);
            let _ = app.emit("tray-open", id);
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// The menu and tooltip follow the tunnel's state.
fn refresh(app: &AppHandle) {
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let Ok(status) = crate::tunnel_status(app.state::<crate::AppState>()) else { return };
    let place = if status.ssh_route.len() > 1 { status.ssh_route.join(" → ") } else { status.server_name.clone() };
    let text = match status.state.as_str() {
        "running" if place.is_empty() => "Подключено".to_owned(),
        "running" => format!("Подключено · {place}"),
        "starting" => "Подключаемся…".to_owned(),
        "reconnecting" => "Переподключаемся…".to_owned(),
        "stopping" => "Отключаемся…".to_owned(),
        "failed" => "Не удалось подключиться".to_owned(),
        _ => "Отключено".to_owned(),
    };
    let look = match status.state.as_str() {
        "running" => Look::On,
        "starting" | "reconnecting" | "unknown" => Look::Connecting,
        _ => Look::Off,
    };
    if let (Ok(mut icon), Some(tray)) = (items.icon.lock(), app.tray_by_id("main")) {
        let step = if look == Look::Connecting { icon.1 + 1 } else { 0 };
        if look != icon.0 || look == Look::Connecting {
            let _ = tray.set_icon(frame(look, step));
        }
        *icon = (look, step);
    }
    let Ok(mut shown) = items.shown.lock() else { return };
    if *shown == text {
        return;
    }
    let idle = matches!(status.state.as_str(), "stopped" | "failed" | "");
    let _ = items.status.set_text(&text);
    let _ = items.connect.set_enabled(idle);
    let _ = items.disconnect.set_enabled(!idle);
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(format!("Secret Tunnel — {text}")));
    }
    *shown = text;
}
