//! Empty Live inbox. Title resources use storage_api, never player messages.
use crate::{entries::Operation, state::Error, status::*};
use runtime::Guest;

/// Public SDK summary size, retained for callers that allocate an empty list.
pub const SUMMARY_SIZE: usize = 64;
/// The voice/text playback screen's attachment capacity, not a title-file cap.
pub const ATTACHMENT_CAP: u32 = 0x499a;

pub(crate) fn invoke(op: Operation, a: &[u32], m: &mut Guest) -> Result<u32, Error> {
    use Operation::*;
    match op {
        MessageEnumerate => {
            m.set_u32(a[2], 0)?;
            Ok(0)
        }
        MessageDetails | MessageDelete => Ok(MESSAGE_INVALID_ID),
        MessageProperty | MessageDownload | MessageDownloadResults => Ok(INVALID_ARGUMENT),
        _ => unreachable!(),
    }
}
