//! 在 Flatpak 的沙盒里运行时的差别。

use std::ffi::OsString;
use std::path::Path;

/// 在 Flatpak 的沙盒里运行（沙盒里总有 `/.flatpak-info`）。
pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// Flatpak 的应用 ID（沙盒里的 `FLATPAK_ID`）；不在沙盒里时为空。
pub fn flatpak_id() -> Option<OsString> {
    std::env::var_os("FLATPAK_ID").filter(|id| !id.is_empty())
}
