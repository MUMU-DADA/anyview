//! "不允许多个实例"。Linux 上每个窗口是一个进程：默认每次从文件管理器打开图片都启动一个新的
//! （新窗口）。勾上这项后，新启动的进程先问问有没有已经开着的：有就把图片交给它、自己马上退出（还没建窗口，
//! 很快），开着的那个在自己的窗口里换图并来到前面。
//!
//! 进程之间用本机的 Unix 套接字：`$XDG_RUNTIME_DIR/anyview-instance.sock`（运行时目录只有自己能访问）。
//! Flatpak 里每次启动的 `$XDG_RUNTIME_DIR` 各自独立，只有 `app/<应用 ID>` 这个子目录是同一应用各次启动共用的，
//! 套接字放在那里。
//! 每个实例启动时都试着占用它，先启动的占着；它退出时删掉，下一个启动的再占。程序崩溃留下的旧文件，
//! 连不上时直接替换。
//!
//! 消息：一行 `anyview-open 1`，然后是激活令牌（Wayland 的 XDG_ACTIVATION_TOKEN，让开着的窗口能来到前面）、
//! 一个 0 字节、要打开的绝对路径（可以为空，只把窗口叫到前面）。对方读完回 `ok`。

use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const HEADER: &[u8] = b"anyview-open 1\n";
/// 一条消息最多多大（路径再长也够）。
const MAX_MESSAGE: u64 = 64 * 1024;
/// 等对方回话最多多久；开着的那个卡住时，新进程自己开窗口。
const TIMEOUT: Duration = Duration::from_secs(3);

/// 套接字的位置；没有运行时目录时为空（这时不支持单实例，照常开新窗口）。
pub fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(socket_in(Path::new(&dir), crate::sandbox::flatpak_id().as_deref()))
}

/// 运行时目录里的套接字；在 Flatpak 里（有应用 ID）时放在同一应用各次启动共用的 `app/<应用 ID>/` 里。
fn socket_in(runtime: &Path, flatpak_id: Option<&std::ffi::OsStr>) -> PathBuf {
    match flatpak_id {
        Some(id) => runtime.join("app").join(id).join("anyview-instance.sock"),
        None => runtime.join("anyview-instance.sock"),
    }
}

/// 开着的那个实例收到的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 要打开的文件或文件夹（绝对路径）；为空时只把窗口叫到前面。
    pub open: Option<PathBuf>,
    /// 激活令牌（可以为空）。
    pub token: Option<String>,
}

/// 把图片交给已经开着的实例。对方回了 `ok` 时返回 true；没有开着的、连不上或不回话时返回 false。
pub fn forward(socket: &Path, open: Option<&Path>) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let token = std::env::var("XDG_ACTIVATION_TOKEN")
        .or_else(|_| std::env::var("DESKTOP_STARTUP_ID"))
        .unwrap_or_default();
    let mut message = HEADER.to_vec();
    message.extend_from_slice(token.as_bytes());
    message.push(0);
    if let Some(path) = open {
        // 对方的当前目录不同：交出绝对路径。
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        message.extend_from_slice(path.as_os_str().as_bytes());
    }
    if stream.write_all(&message).is_err() || stream.shutdown(Shutdown::Write).is_err() {
        return false;
    }
    let mut reply = String::new();
    stream.read_to_string(&mut reply).is_ok() && reply.trim() == "ok"
}

/// 占用套接字、在后台线程接收别的进程交来的图片，收到的请求交给 `deliver`（在后台线程调用）。
/// 已经有开着的实例占着时返回 false（这个进程不接收）。
pub fn listen(socket: &Path, deliver: impl Fn(Request) + Send + 'static) -> bool {
    let listener = match UnixListener::bind(socket) {
        Ok(listener) => listener,
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            if UnixStream::connect(socket).is_ok() {
                return false; // 先启动的实例还开着
            }
            // 崩溃留下的旧文件：替换掉。
            let _ = std::fs::remove_file(socket);
            match UnixListener::bind(socket) {
                Ok(listener) => listener,
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };
    let spawned = thread::Builder::new()
        .name("anyview-instance".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                if let Some(request) = read_request(&mut stream) {
                    let _ = stream.write_all(b"ok\n");
                    deliver(request);
                }
            }
        });
    spawned.is_ok()
}

/// 退出时删掉自己占着的套接字，下一个启动的实例再占。
pub fn release(socket: &Path) {
    let _ = std::fs::remove_file(socket);
}

fn read_request(stream: &mut UnixStream) -> Option<Request> {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let mut message = Vec::new();
    Read::by_ref(stream)
        .take(MAX_MESSAGE)
        .read_to_end(&mut message)
        .ok()?;
    parse(&message)
}

fn parse(message: &[u8]) -> Option<Request> {
    let body = message.strip_prefix(HEADER)?;
    let split = body.iter().position(|&b| b == 0)?;
    let (token, path) = (&body[..split], &body[split + 1..]);
    let token = String::from_utf8(token.to_vec()).ok()?;
    Some(Request {
        open: (!path.is_empty()).then(|| PathBuf::from(OsString::from_vec(path.to_vec()))),
        token: (!token.is_empty()).then_some(token),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn flatpak_socket_is_in_the_shared_app_directory() {
        let runtime = Path::new("/run/user/1000");
        assert_eq!(socket_in(runtime, None), Path::new("/run/user/1000/anyview-instance.sock"));
        assert_eq!(
            socket_in(runtime, Some(std::ffi::OsStr::new("local.anyview.Anyview"))),
            Path::new("/run/user/1000/app/local.anyview.Anyview/anyview-instance.sock")
        );
    }

    #[test]
    fn messages_round_trip() {
        let mut message = HEADER.to_vec();
        message.extend_from_slice(b"token-1\0/tmp/a b/\xe4\xb8\xad.png");
        assert_eq!(
            parse(&message),
            Some(Request {
                open: Some(PathBuf::from("/tmp/a b/中.png")),
                token: Some("token-1".into()),
            })
        );
        let mut empty = HEADER.to_vec();
        empty.push(0);
        assert_eq!(
            parse(&empty),
            Some(Request {
                open: None,
                token: None
            })
        );
        assert_eq!(parse(b"something else"), None);
    }

    #[test]
    fn a_second_process_hands_its_image_to_the_first() {
        let dir = std::env::temp_dir().join(format!("anyview-instance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("instance.sock");
        // 没有开着的：交不出去。
        assert!(!forward(&socket, Some(Path::new("/tmp/x.png"))));
        // 崩溃留下的旧文件不妨碍占用。
        std::fs::write(&socket, b"").unwrap();
        let (tx, rx) = mpsc::channel();
        assert!(listen(&socket, move |request| {
            let _ = tx.send(request);
        }));
        // 已经有人占着：第二个不接收。
        assert!(!listen(&socket, |_| {}));
        assert!(forward(&socket, Some(Path::new("/tmp/x.png"))));
        let request = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(request.open, Some(PathBuf::from("/tmp/x.png")));
        release(&socket);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
