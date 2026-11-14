//! 记住窗口大小：关闭窗口时把大小和是否最大化写进 `$XDG_STATE_HOME/anyview/window.json`
//! （默认 `~/.local/state/anyview/window.json`），下次启动时恢复；第一次启动约占显示器的 70%。
//! Wayland 不允许程序摆放窗口，所以只记大小，不记位置。

use std::path::PathBuf;

use gtk::gdk;
use gtk::prelude::*;

fn state_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    Some(base.join("anyview").join("window.json"))
}

/// 恢复上次的大小；没有记录时约占显示器的 70%。
pub fn restore(window: &gtk::ApplicationWindow) {
    let saved = state_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    if let Some(state) = saved {
        let width = state["width"].as_i64().unwrap_or(0);
        let height = state["height"].as_i64().unwrap_or(0);
        if (480..=16384).contains(&width) && (360..=16384).contains(&height) {
            window.set_default_size(width as i32, height as i32);
        }
        if state["maximized"].as_bool() == Some(true) {
            window.maximize();
        }
        return;
    }
    let monitor = gdk::Display::default()
        .and_then(|display| display.monitors().item(0))
        .and_then(|item| item.downcast::<gdk::Monitor>().ok());
    if let Some(monitor) = monitor {
        let geometry = monitor.geometry();
        window.set_default_size(
            ((f64::from(geometry.width()) * 0.7) as i32).max(480),
            ((f64::from(geometry.height()) * 0.7) as i32).max(360),
        );
    }
}

/// 记下现在的大小（最大化时记还原后的大小）和是否最大化。写不进去时不提示，下次按默认大小打开。
pub fn save(window: &gtk::ApplicationWindow) {
    let Some(path) = state_file() else { return };
    // GTK4 的默认大小随用户调整窗口而更新，就是还原后的大小。
    let (width, height) = window.default_size();
    let state = serde_json::json!({
        "width": width,
        "height": height,
        "maximized": window.is_maximized(),
    });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, state.to_string());
}
