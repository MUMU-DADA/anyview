//! 翻页音效（设置"翻页时播放音效"）：声音由核心合成（WAV），这里只负责放出来。
//!
//! 这台机器的 GTK 不一定带媒体后端（没有 GStreamer 时 GtkMediaFile 放不出声），所以先找系统里的播放命令：
//! PipeWire 的 pw-play、PulseAudio 的 paplay、ALSA 的 aplay，都没有时再用 GtkMediaFile。
//! WAV 第一次用时写到运行时目录（$XDG_RUNTIME_DIR，关机就没了）。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gtk::prelude::*;
use gtk::glib;

#[derive(Default)]
pub struct PageSound {
    /// 写好的 WAV 文件和选好的播放方式；第一次放时准备。
    ready: RefCell<Option<(PathBuf, Backend)>>,
    media: RefCell<Option<gtk::MediaFile>>,
}

#[derive(Clone, Debug)]
enum Backend {
    Command(PathBuf),
    Gtk,
}

impl PageSound {
    /// 放一次（上一次还没放完时各放各的）。返回用的是哪种方式（自测记录用）。
    pub fn play(&self) -> String {
        let Some((file, backend)) = self.prepare() else {
            return "unavailable".into();
        };
        match backend {
            Backend::Command(program) => {
                let spawned = Command::new(&program)
                    .arg(&file)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                if let Ok(mut child) = spawned {
                    // 在后台等它结束，不留僵尸进程。
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                }
                program
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }
            Backend::Gtk => {
                let mut media = self.media.borrow_mut();
                let media = media.get_or_insert_with(|| gtk::MediaFile::for_filename(&file));
                media.seek(0);
                media.play();
                "gtk".into()
            }
        }
    }

    /// 自测用：准备好（写 WAV、选播放方式）但不放，返回方式和 WAV 文件。
    pub fn describe(&self) -> Option<(String, PathBuf)> {
        let (file, backend) = self.prepare()?;
        let name = match backend {
            Backend::Command(p) => p.file_name()?.to_string_lossy().into_owned(),
            Backend::Gtk => "gtk".into(),
        };
        Some((name, file))
    }

    fn prepare(&self) -> Option<(PathBuf, Backend)> {
        if let Some(ready) = self.ready.borrow().clone() {
            return Some(ready);
        }
        let file = glib::user_runtime_dir().join("anyview-page-turn.wav");
        std::fs::write(&file, anyview_core::transition::page_sound_wav()).ok()?;
        let backend = ["pw-play", "paplay", "aplay"]
            .iter()
            .find_map(|name| find_in_path(name))
            .map(Backend::Command)
            .unwrap_or(Backend::Gtk);
        *self.ready.borrow_mut() = Some((file.clone(), backend.clone()));
        Some((file, backend))
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
