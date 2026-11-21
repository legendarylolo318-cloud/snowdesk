//! Single-instance IPC. Unix socket on Linux; on Windows a localhost TCP
//! port stored in a file (std has no named pipes; avoids an extra dependency).

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

/// Read one command line from a client. A client that connects and then
/// stays silent must not block the listener, so reads time out, and lines
/// are capped in length.
fn read_cmd(s: impl Read) -> Option<IpcCmd> {
    let mut line = String::new();
    BufReader::new(s.take(64)).read_line(&mut line).ok()?;
    IpcCmd::parse(&line)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCmd {
    TogglePlay,
    Pause,
    Clear,
    NextTrack,
    Show,
    Quit,
    ExportPicture,
    ExportScene,
}

impl IpcCmd {
    pub fn name(self) -> &'static str {
        match self {
            Self::TogglePlay => "toggle-play",
            Self::Pause => "pause",
            Self::Clear => "clear",
            Self::NextTrack => "next-track",
            Self::Show => "show",
            Self::Quit => "quit",
            Self::ExportPicture => "export-picture",
            Self::ExportScene => "export-scene",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "toggle-play" => Self::TogglePlay,
            "pause" => Self::Pause,
            "clear" => Self::Clear,
            "next-track" => Self::NextTrack,
            "show" => Self::Show,
            "quit" => Self::Quit,
            "export-picture" => Self::ExportPicture,
            "export-scene" => Self::ExportScene,
            _ => return None,
        })
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub fn socket_path() -> PathBuf {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join("snowdesk.sock")
    }

    pub fn send(cmd: IpcCmd) -> std::io::Result<()> {
        let mut s = UnixStream::connect(socket_path())?;
        writeln!(s, "{}", cmd.name())
    }

    pub fn listen(on_cmd: impl Fn(IpcCmd) + Send + 'static) -> std::io::Result<()> {
        let path = socket_path();
        // A live instance answers; a stale socket does not.
        if UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "already running",
            ));
        }
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path)?;
        std::thread::Builder::new()
            .name("ipc".into())
            .spawn(move || {
                for s in l.incoming().flatten() {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                    if let Some(c) = read_cmd(s) {
                        on_cmd(c);
                    }
                }
            })?;
        Ok(())
    }

    pub fn cleanup() {
        let _ = std::fs::remove_file(socket_path());
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;

    fn port_file() -> PathBuf {
        crate::config::Config::dir().join("ipc.port")
    }

    pub fn send(cmd: IpcCmd) -> std::io::Result<()> {
        let port: u16 = std::fs::read_to_string(port_file())?
            .trim()
            .parse()
            .map_err(|_| std::io::Error::other("bad port"))?;
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        writeln!(s, "{}", cmd.name())
    }

    pub fn listen(on_cmd: impl Fn(IpcCmd) + Send + 'static) -> std::io::Result<()> {
        if send(IpcCmd::Show).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "already running",
            ));
        }
        let l = TcpListener::bind(("127.0.0.1", 0))?;
        let _ = std::fs::create_dir_all(crate::config::Config::dir());
        std::fs::write(port_file(), l.local_addr()?.port().to_string())?;
        std::thread::Builder::new()
            .name("ipc".into())
            .spawn(move || {
                for s in l.incoming().flatten() {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                    if let Some(c) = read_cmd(s) {
                        on_cmd(c);
                    }
                }
            })?;
        Ok(())
    }

    pub fn cleanup() {
        let _ = std::fs::remove_file(port_file());
    }
}

pub use imp::{cleanup, listen, send};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_round_trip() {
        for c in [
            IpcCmd::TogglePlay,
            IpcCmd::Pause,
            IpcCmd::Clear,
            IpcCmd::NextTrack,
            IpcCmd::Show,
            IpcCmd::Quit,
            IpcCmd::ExportPicture,
            IpcCmd::ExportScene,
        ] {
            assert_eq!(IpcCmd::parse(c.name()), Some(c));
        }
        assert_eq!(IpcCmd::parse("nope"), None);
    }

    #[test]
    fn oversized_or_garbage_input_is_ignored() {
        assert_eq!(read_cmd(&b"quit\n"[..]), Some(IpcCmd::Quit));
        let long = "x".repeat(10_000);
        assert_eq!(read_cmd(long.as_bytes()), None);
        assert_eq!(read_cmd(&[0xff, 0xfe, 0x00][..]), None);
    }

    /// Regression: a client that connects and never writes used to block
    /// every later command forever.
    #[cfg(unix)]
    #[test]
    fn silent_client_does_not_block_listener() {
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::sync::mpsc;
        let dir = std::env::temp_dir().join(format!("snowdesk-ipc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.sock");
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                if let Some(c) = read_cmd(s) {
                    let _ = tx.send(c);
                }
            }
        });
        let _silent = UnixStream::connect(&path).unwrap();
        let mut s = UnixStream::connect(&path).unwrap();
        writeln!(s, "pause").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)), Ok(IpcCmd::Pause));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
