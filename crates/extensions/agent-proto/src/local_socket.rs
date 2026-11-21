//! Pathname AF_UNIX streams shared by the game and editor protocols.

use std::{io, path::Path};

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{UnixListener, UnixStream};

/// Replace a stale socket, keeping active listeners and other files intact.
pub fn listen(path: &Path) -> io::Result<UnixListener> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            if !crate::discover::is_socket(path) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} is not a socket", path.display()),
                ));
            }
            match UnixStream::connect(path) {
                Ok(_) => return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("{} already has a listener", path.display()),
                )),
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(path)?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(listener)
}
