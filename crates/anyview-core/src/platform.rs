//! 平台标识。核心按平台给出默认键位和措辞（docs/ACTIONS.md），但不依赖任何平台 API。

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    /// 编译目标所在的平台。
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// "打开图片"（`open_file`）的主快捷键：Windows、Linux 默认为 F2，macOS 为 ⌘O。
    pub fn open_file_shortcut(self) -> &'static str {
        match self {
            Self::Macos => "⌘O",
            Self::Windows | Self::Linux => "F2",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_matches_build_target() {
        let expected = if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        };
        assert_eq!(Platform::current(), expected);
    }
}
