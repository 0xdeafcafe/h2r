//! Object-manager references by handle (246) and their release (250).
//!
//! XAPI's SetThreadPriority (0x2d1fb6) is the observed caller:
//! `ObReferenceObjectByHandle(handle, PsThreadObjectType, &thread)`, then
//! `KeSetBasePriorityThread(thread, increment)` and `ObfDereferenceObject(thread)`.
//! A reference pins a kernel-owned dispatcher object exactly like a host
//! `DispatcherReference`: its pool storage is not reused until the guest
//! dereferences it. Guest references are counted separately so a guest
//! dereference can never release a host pin.
use crate::{
    Action, Core,
    abi::{Args, Value},
    ord, status,
    sync::Object,
};
use runtime::{Guest, GuestFault, guest::Access};

/// NtCurrentThread().
pub(crate) const CURRENT_THREAD: u32 = 0xffff_fffe;

impl Core {
    pub(crate) fn object_call(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        tid: u32,
    ) -> Result<Action, GuestFault> {
        match a.export.ordinal {
            ord::OB_REFERENCE_OBJECT_BY_HANDLE => {
                let (handle, object_type, out) = (a.get(0), a.get(1), a.get(2));
                memory.check(out, 4, Access::Write, 1)?;
                let object = if handle == CURRENT_THREAD {
                    match self.threads.get(&tid) {
                        Some(record) => record.object,
                        None => {
                            return Ok(Action::Stop(
                                "current-thread pseudo-handle requires Machine",
                            ));
                        }
                    }
                } else {
                    match self.handles.get(handle) {
                        Ok(object) => object,
                        Err(s) => return Ok(Action::status(s)),
                    }
                };
                if self.files.contains_key(&object) || self.links.contains_key(&object) {
                    return Ok(Action::Stop("file and link object references unsupported"));
                }
                let Some(kind) = self.ensure_object(memory, object)? else {
                    return Ok(Action::Stop("invalid dispatcher header for handle"));
                };
                // The type argument is the address of an OBJECT_TYPE DATA export.
                // Only those the image imports can be recognised; a null type
                // accepts any object, as on retail.
                let matches = if object_type == 0 {
                    true
                } else if Some(&object_type) == self.data.get(&ord::PS_THREAD_OBJECT_TYPE) {
                    matches!(kind, Object::Thread { .. })
                } else if Some(&object_type) == self.data.get(&ord::EX_EVENT_OBJECT_TYPE) {
                    matches!(kind, Object::Event { .. })
                } else {
                    return Ok(Action::Stop("object type pointer unrecognised"));
                };
                if !matches {
                    return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
                }
                let guest = self.guest_references.entry(object).or_default();
                let Some(next) = guest.checked_add(1) else {
                    return Ok(Action::Stop("object reference count overflow"));
                };
                *guest = next;
                *self.references.entry(object).or_default() += 1;
                memory.set_u32(out, object)?;
                Ok(Action::status(status::SUCCESS))
            }
            ord::OBF_DEREFERENCE_OBJECT => {
                let object = a.get(0);
                let Some(count) = self.guest_references.get_mut(&object) else {
                    return Ok(Action::Stop(
                        "dereference of an object the guest did not reference",
                    ));
                };
                *count -= 1;
                if *count == 0 {
                    self.guest_references.remove(&object);
                }
                self.release_reference(object);
                Ok(Action::Complete(Value::Void))
            }
            _ => Ok(Action::Stop("declared export has no handler")),
        }
    }
}
