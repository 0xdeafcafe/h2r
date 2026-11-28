//! The unit mouth (`0x18c6a0`) as the sound owners call it: the engine's
//! port over guest memory ([`engine::sim_sound::mouth`]), with the
//! resource cache's touch (`0x12de70`) made by the guest when its answer
//! needs I/O (docs/engine/sound-instances.md, "Owners").

use engine::entry::{Cx, EntryError, GuestCall};
use engine::sim_sound::mouth::{self, CACHE_TOUCH, Call, TOUCH_FLAGS};

/// `0x18c6a0(args)`: the unit's mouth for one tick of its line.
pub async fn run(cx: &Cx, args: [u32; 6]) -> Result<(), EntryError> {
    let call = Call::from_args(args);
    let Some(pending) = cx.write(|m| mouth::run(m, &call))? else {
        return Ok(());
    };
    let regs = cx
        .call(
            GuestCall::new(CACHE_TOUCH)
                .ecx(pending.resource())
                .eax(TOUCH_FLAGS),
        )
        .await?;
    cx.write(|m| mouth::finish(m, &call, &pending, regs.eax & 0xff != 0))?;
    Ok(())
}
