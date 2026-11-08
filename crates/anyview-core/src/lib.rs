//! Anyview 共用核心：与平台无关的业务逻辑（docs/ARCHITECTURE.md）。
//!
//! 负责打开图片/文件夹/压缩包、自然排序、切换、后台解码、缓存与预加载、
//! 查看计算（缩放、平移、旋转、翻转、显示方式）；动作目录、默认键位与菜单结构；悬浮界面的规则与文字。

// 文字宏 tr!、msg! 在 i18n 里定义，放在最前面，后面的模块都能直接用。
#[macro_use]
pub mod i18n;
pub mod actions;
pub mod anim;
pub mod archive;
mod api;
pub mod avif;
pub mod cache;
pub mod clip;
pub mod collection;
pub mod color;
pub mod dds;
pub mod decode;
pub mod effects;
pub mod exact;
pub mod exif;
pub mod formats;
pub mod generation;
pub mod hdr;
#[cfg(not(target_os = "macos"))]
pub mod heif;
pub mod help;
pub mod jp2;
pub mod jpeg;
pub mod jxl;
pub mod jxr;
pub mod licenses;
pub mod natural;
pub mod paths;
pub mod overlay;
pub mod packaging;
pub mod pcx;
pub mod picker;
pub mod platform;
pub mod positions;
pub mod pool;
pub mod proxy;
pub mod psd;
mod psd_layers;
pub mod radiance;
pub mod raw;
pub mod recent;
pub mod settings;
pub mod spread;
pub mod svg;
pub mod thumbnail;
pub mod transition;
pub mod view;
pub mod winshell;

pub use api::{
    ActionOutcome, ActionState, AppCore, Command, CoreListener, Event, Navigation, NoticeKind,
    AnimationInfo, PickerList, PickerSortOption, SettingsApplied, SettingsPageView, UiRequest, UiSettings,
};
pub use decode::{DecodedImage, ImageInfo};
pub use i18n::Lang;
pub use platform::Platform;
pub use view::{
    DisplayMode, EdgeMode, Interpolation, Layout, Rect, SwipePhase, fit_rect, zoom_text,
};

/// 核心版本号，取自 Cargo.toml。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 空态提示：按平台显示"打开图片"的快捷键。
pub fn empty_state_hint(platform: Platform, lang: Lang) -> String {
    i18n::text(i18n::Key::EmptyStateHint, lang).replace("{key}", platform.open_file_shortcut())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn empty_state_hint_uses_platform_shortcut() {
        assert_eq!(
            empty_state_hint(Platform::Macos, Lang::ZhHans),
            "将图片拖到窗口中，或按 ⌘O 打开图片"
        );
        assert!(empty_state_hint(Platform::Linux, Lang::En).contains("F2"));
        assert!(empty_state_hint(Platform::Windows, Lang::ZhHans).contains("F2"));
    }
}
