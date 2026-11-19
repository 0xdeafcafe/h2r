//! `H2_SAVE_TRACE=1`: one stderr line per guest file call on the save drive
//! (`U:`, the data partition's `UDATA`), with its arguments, its NTSTATUS
//! and the guest code that asked for it.
//!
//! Written to settle "CAN'T CREATE PROFILE" (ui error 0xc, the game's error
//! 4 from the saved-game files code): the game shows it when creating the
//! save (`0x215c00`: XCreateSaveGame CREATE_NEW, then the `profile` file
//! with CREATE_ALWAYS) or writing the new profile (`0x2ae090` opens
//! `profile` OPEN_EXISTING, GENERIC_WRITE, share 0) fails while the
//! free-space query says there is room. Every one of those is an
//! NtCreateFile/NtOpenFile on `U:`, so one run with this set names the call
//! and the status that failed it.
//!
//! Opens are traced by name. Calls on a handle are traced when the handle
//! was opened under `U:` while tracing (reads and writes only when they
//! fail, so a profile's 4 MB `auxilary.bin` stays a few lines). Each line
//! ends with the call's return address and up to five more return
//! addresses found on the guest stack (a call instruction right before
//! them), innermost first, which name the XAPI wrapper and the game
//! function behind it.
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use runtime::Guest;

use super::object_name;
use crate::abi::{Args, Value};
use crate::{Action, ord};

/// Whether `H2_SAVE_TRACE` asks for the trace (set, not empty, not `0`).
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| requested(std::env::var("H2_SAVE_TRACE").ok().as_deref()))
}

fn requested(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether an object name is on the save drive: `\??\U:` (any case) or a
/// path through the data partition's `UDATA`.
pub(crate) fn is_save_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("\\??\\u:") || lower.contains("\\udata\\") || lower.ends_with("\\udata")
}

/// Guest file handles opened under `U:` while tracing, and their names.
fn handles() -> &'static Mutex<BTreeMap<u32, String>> {
    static HANDLES: OnceLock<Mutex<BTreeMap<u32, String>>> = OnceLock::new();
    HANDLES.get_or_init(Default::default)
}

fn handle_name(handle: u32) -> Option<String> {
    handles()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&handle)
        .cloned()
}

/// What a traced call was about, taken before it runs.
pub(crate) struct Pending {
    call: &'static str,
    name: String,
    detail: String,
    /// The call opens a handle (written to `a.get(0)` on success).
    opens: bool,
}

fn call_name(ordinal: u32) -> &'static str {
    match ordinal {
        ord::NT_CREATE_FILE => "NtCreateFile",
        ord::NT_OPEN_FILE => "NtOpenFile",
        ord::NT_CLOSE => "NtClose",
        ord::NT_READ_FILE => "NtReadFile",
        ord::NT_WRITE_FILE => "NtWriteFile",
        ord::NT_FLUSH_BUFFERS_FILE => "NtFlushBuffersFile",
        ord::NT_QUERY_DIRECTORY_FILE => "NtQueryDirectoryFile",
        ord::NT_QUERY_INFORMATION_FILE => "NtQueryInformationFile",
        ord::NT_SET_INFORMATION_FILE => "NtSetInformationFile",
        ord::NT_QUERY_VOLUME_INFORMATION_FILE => "NtQueryVolumeInformationFile",
        ord::NT_QUERY_FULL_ATTRIBUTES_FILE => "NtQueryFullAttributesFile",
        ord::NT_DUPLICATE_OBJECT => "NtDuplicateObject",
        _ => "file call",
    }
}

/// The name an OBJECT_ATTRIBUTES gives (`\??\` names are absolute by
/// now), with a root handle's traced name in front when it is relative to one.
fn attributes_name(memory: &Guest, attributes: u32) -> Option<String> {
    let name = object_name(memory, attributes).ok()?.ok()?;
    if name.root == 0 {
        return Some(name.path);
    }
    Some(match handle_name(name.root) {
        Some(root) => format!("{root}\\{}", name.path),
        None => format!("<handle {:#x}>\\{}", name.root, name.path),
    })
}

/// Before the call: `Some` when it is one to trace.
pub(crate) fn before(memory: &Guest, a: &Args) -> Option<Pending> {
    let ordinal = a.export.ordinal;
    let call = call_name(ordinal);
    match ordinal {
        ord::NT_CREATE_FILE | ord::NT_OPEN_FILE | ord::NT_QUERY_FULL_ATTRIBUTES_FILE => {
            let attributes = if ordinal == ord::NT_QUERY_FULL_ATTRIBUTES_FILE {
                a.get(0)
            } else {
                a.get(2)
            };
            let name = attributes_name(memory, attributes)?;
            if !is_save_path(&name) {
                return None;
            }
            let detail = match ordinal {
                ord::NT_CREATE_FILE => {
                    let allocation = match a.get(4) {
                        0 => "none".to_string(),
                        p => memory
                            .u32(p)
                            .map_or_else(|_| format!("@{p:#x}"), |v| v.to_string()),
                    };
                    format!(
                        "access={:#x} share={:#x} disposition={} options={:#x} attributes={:#x} allocation={allocation}",
                        a.get(1),
                        a.get(6),
                        a.get(7),
                        a.get(8),
                        a.get(5)
                    )
                }
                ord::NT_OPEN_FILE => format!(
                    "access={:#x} share={:#x} options={:#x}",
                    a.get(1),
                    a.get(4),
                    a.get(5)
                ),
                _ => String::new(),
            };
            Some(Pending {
                call,
                name,
                detail,
                opens: ordinal != ord::NT_QUERY_FULL_ATTRIBUTES_FILE,
            })
        }
        _ if super::is_file_export(ordinal)
            || matches!(ordinal, ord::NT_CLOSE | ord::NT_DUPLICATE_OBJECT) =>
        {
            let name = handle_name(a.get(0))?;
            let detail = match ordinal {
                ord::NT_READ_FILE | ord::NT_WRITE_FILE => format!("length={:#x}", a.get(6)),
                ord::NT_QUERY_INFORMATION_FILE
                | ord::NT_SET_INFORMATION_FILE
                | ord::NT_QUERY_VOLUME_INFORMATION_FILE => format!("class={}", a.get(4)),
                _ => String::new(),
            };
            if ordinal == ord::NT_CLOSE {
                handles()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&a.get(0));
            }
            Some(Pending {
                call,
                name,
                detail,
                opens: false,
            })
        }
        _ => None,
    }
}

/// The call's NTSTATUS, when it completed with one.
fn status_of(result: &Result<Action, runtime::GuestFault>) -> Result<u32, String> {
    match result {
        Ok(Action::Complete(Value::U32(s)) | Action::Block(Value::U32(s))) => Ok(*s),
        Ok(Action::Stop(reason)) => Err(format!("stop: {reason}")),
        Ok(_) => Err("no status".into()),
        Err(fault) => Err(format!("guest fault: {fault:?}")),
    }
}

/// Up to `max` return addresses on the guest stack above `stack`: words
/// in the image range right after a `call rel32` or `call [abs32]`.
fn callers(memory: &Guest, stack: u32, max: usize) -> Vec<u32> {
    let mut found = Vec::new();
    for i in 1..256u32 {
        let Ok(word) = memory.u32(stack.wrapping_add(i * 4)) else {
            break;
        };
        if !(0x0001_1000..0x0050_0000).contains(&word) {
            continue;
        }
        let rel = memory.u8(word.wrapping_sub(5)).is_ok_and(|b| b == 0xe8);
        let abs = memory
            .read::<2>(word.wrapping_sub(6))
            .is_ok_and(|b| b == [0xff, 0x15]);
        if rel || abs {
            found.push(word);
            if found.len() == max {
                break;
            }
        }
    }
    found
}

/// One trace line.
pub(crate) fn line(
    call: &str,
    name: &str,
    detail: &str,
    outcome: &Result<u32, String>,
    extra: &str,
    return_pc: u32,
    callers: &[u32],
) -> String {
    let outcome = match outcome {
        Ok(s) => format!("status={s:#010x}"),
        Err(e) => e.clone(),
    };
    let mut out = format!("save-trace: {call} {name}");
    for part in [detail, outcome.as_str(), extra] {
        if !part.is_empty() {
            out.push(' ');
            out.push_str(part);
        }
    }
    out.push_str(&format!(" from {return_pc:#x}"));
    for c in callers {
        out.push_str(&format!(" < {c:#x}"));
    }
    out
}

/// After the call: print its line, and remember a handle it opened.
pub(crate) fn after(
    memory: &Guest,
    a: &Args,
    pending: Pending,
    result: &Result<Action, runtime::GuestFault>,
) {
    let outcome = status_of(result);
    let failed = !matches!(outcome, Ok(s) if s & 0x8000_0000 == 0);
    if matches!(a.export.ordinal, ord::NT_READ_FILE | ord::NT_WRITE_FILE) && !failed {
        return;
    }
    let mut extra = String::new();
    if pending.opens && !failed {
        if let Ok(handle) = memory.u32(a.get(0)) {
            handles()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(handle, pending.name.clone());
            extra = format!("handle={handle:#x}");
        }
        if let Ok(information) = memory.u32(a.get(3).wrapping_add(4)) {
            extra.push_str(&format!(" information={information}"));
        }
    }
    let callers = callers(memory, a.stack, 5);
    eprintln!(
        "{}",
        line(
            pending.call,
            &pending.name,
            &pending.detail,
            &outcome,
            &extra,
            a.return_pc,
            &callers
        )
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_only_when_set_and_not_zero() {
        assert!(!requested(None));
        assert!(!requested(Some("")));
        assert!(!requested(Some("0")));
        assert!(requested(Some("1")));
        assert!(requested(Some("yes")));
    }

    #[test]
    fn save_paths_are_u_and_udata_only() {
        assert!(is_save_path("\\??\\u:\\0123456789AB\\SaveMeta.xbx"));
        assert!(is_save_path("\\??\\U:\\"));
        assert!(is_save_path(
            "\\Device\\Harddisk0\\Partition1\\UDATA\\4D530064\\0123456789AB\\profile"
        ));
        assert!(is_save_path("\\Device\\Harddisk0\\Partition1\\UDATA"));
        assert!(!is_save_path("\\??\\t:\\$c\\4d53006400000101\\turf.map"));
        assert!(!is_save_path("\\??\\d:\\maps\\mainmenu.map"));
        assert!(!is_save_path("\\??\\z:\\cache000.map"));
    }

    #[test]
    fn a_line_names_the_call_its_status_and_its_callers() {
        let l = line(
            "NtOpenFile",
            "\\??\\u:\\0123456789AB\\profile",
            "access=0x40100000 share=0x0 options=0x60",
            &Ok(0xc000_0043),
            "",
            0x2d2878,
            &[0x136a32, 0x2ae0aa],
        );
        assert_eq!(
            l,
            "save-trace: NtOpenFile \\??\\u:\\0123456789AB\\profile \
             access=0x40100000 share=0x0 options=0x60 status=0xc0000043 \
             from 0x2d2878 < 0x136a32 < 0x2ae0aa"
        );
        let l = line("NtClose", "u", "", &Err("stop: x".into()), "", 1, &[]);
        assert_eq!(l, "save-trace: NtClose u stop: x from 0x1");
    }
}
