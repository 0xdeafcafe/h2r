//! The Unix socket: one thread accepting, two per connection (lines in,
//! lines out). Connection threads only queue; the game thread answers.

use std::io::{BufRead, BufReader, Read as _, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use crate::Agent;

/// Longest request line accepted (1 MiB); longer closes the connection.
const MAX_LINE: usize = 1 << 20;

/// The listening socket. Dropping it stops accepting and removes the file;
/// open connections end when the game exits.
pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Listens at `path`. A socket file left by a run that's gone is
    /// replaced; one that something still answers on is an error.
    pub fn listen(path: &Path, agent: Agent) -> std::io::Result<Server> {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            use std::os::unix::fs::FileTypeExt;
            if !meta.file_type().is_socket() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "{}: a file that isn't a socket is there; not replacing it",
                        path.display()
                    ),
                ));
            }
            if UnixStream::connect(path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("{}: another game is listening there", path.display()),
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        thread::Builder::new()
            .name("agent-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if s.load(Ordering::Relaxed) {
                        break;
                    }
                    match stream {
                        Ok(stream) => serve(stream, agent.clone()),
                        Err(e) => eprintln!("agent: accept: {e}"),
                    }
                }
            })?;
        Ok(Server {
            path: path.to_owned(),
            stop,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept loop so it sees the flag.
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(stream: UnixStream, agent: Agent) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel::<String>();
    let conn = agent.connect(tx);
    let _ = thread::Builder::new()
        .name(format!("agent-out-{conn}"))
        .spawn(move || {
            for line in rx {
                if out
                    .write_all(line.as_bytes())
                    .and_then(|_| out.write_all(b"\n"))
                    .is_err()
                {
                    break;
                }
            }
        });
    let _ = thread::Builder::new()
        .name(format!("agent-in-{conn}"))
        .spawn(move || {
            let mut r = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                match (&mut r).take(MAX_LINE as u64 + 1).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(n) if n > MAX_LINE => break,
                    Ok(_) => agent.submit(conn, &line),
                }
            }
            // Dropping the connection's sender ends the writer thread.
            agent.disconnect(conn);
        });
}
