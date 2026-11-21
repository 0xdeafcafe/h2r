use std::collections::BTreeMap;

/// On macOS each command has two seconds and a bounded combined output buffer.
/// Keep command failures and timeouts in the report alongside partial output.
pub fn diagnostics(pid: u32) -> BTreeMap<String, String> {
    #[cfg(target_os = "macos")]
    {
        let pid = pid.to_string();
        BTreeMap::from([
            (
                "vmmap".into(),
                capture("/usr/bin/vmmap", &["--summary", &pid]),
            ),
            ("footprint".into(), capture("/usr/bin/footprint", &[&pid])),
        ])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        BTreeMap::new()
    }
}

#[cfg(target_os = "macos")]
fn capture(program: &str, args: &[&str]) -> String {
    use std::io::{self, Read};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn run(program: &str, args: &[&str]) -> io::Result<String> {
        const MAX_OUTPUT: usize = 256 * 1024;
        let (mut read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(write.try_clone()?)))
            .stderr(Stdio::from(OwnedFd::from(write)))
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut output = Vec::new();
        let mut buf = [0; 8192];
        let mut truncated = false;
        let mut status = None;
        let note = loop {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break "timed out".to_owned();
            }
            match read.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let keep = n.min(MAX_OUTPUT - output.len());
                    output.extend_from_slice(&buf[..keep]);
                    truncated |= keep < n;
                    continue;
                }
                Ok(_) => (),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break e.to_string();
                }
            }
            // Drain the final bytes written before exiting.
            if let Some(exit) = status {
                break format!("{exit}");
            }
            status = child.try_wait()?;
            if status.is_none() {
                std::thread::sleep(Duration::from_millis(15));
            }
        };
        Ok(format!(
            "{}\n[{}{}]",
            String::from_utf8_lossy(&output),
            note,
            if truncated { ", output truncated" } else { "" }
        ))
    }
    run(program, args).unwrap_or_else(|e| format!("{program}: {e}"))
}
