//! Access violations delivered by Windows' vectored exception dispatcher.
use super::{Error, Fault, dispatch};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
    EXCEPTION_POINTERS,
};

pub(super) fn install() -> Result<(), Error> {
    static INSTALLED: OnceLock<Result<(), Error>> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        // SAFETY: handler belongs to the host executable for its entire lifetime.
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
        if handle.is_null() {
            Err(Error::Install)
        } else {
            Ok(())
        }
    })
}

unsafe extern "system" fn handler(exception: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows supplies a valid record and a writable x64 CONTEXT
    // for the interrupted thread for the duration of this call.
    let record = unsafe { &*(*exception).ExceptionRecord };
    if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION
        || record.ExceptionFlags != 0
        || record.NumberParameters < 2
        || !matches!(record.ExceptionInformation[0], 0 | 1)
    {
        // Let the next handler see all other exceptions, including execution
        // faults. Regions own data reads and writes only.
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let context = unsafe { &mut *(*exception).ContextRecord };
    let mut fault = Fault {
        address: record.ExceptionInformation[1],
        pc: context.Rip as usize,
    };
    if dispatch(&mut fault) {
        context.Rip = fault.pc as u64;
        EXCEPTION_CONTINUE_EXECUTION
    } else {
        EXCEPTION_CONTINUE_SEARCH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, EXCEPTION_RECORD};

    #[test]
    fn only_owned_continuable_data_faults_update_the_saved_pc() {
        let base = 0x1200;
        let next = 0x4000;
        let _region = crate::register_region(
            base,
            1,
            |fault, pc| {
                fault.resume_at(pc);
                true
            },
            next,
        )
        .unwrap();
        let mut record = EXCEPTION_RECORD {
            ExceptionCode: EXCEPTION_ACCESS_VIOLATION,
            NumberParameters: 2,
            ..Default::default()
        };
        let mut context = CONTEXT::default();
        let mut exception = EXCEPTION_POINTERS {
            ExceptionRecord: &mut record,
            ContextRecord: &mut context,
        };
        for (address, access, flags, expected) in [
            (base, 0, 0, EXCEPTION_CONTINUE_EXECUTION),
            (base, 1, 0, EXCEPTION_CONTINUE_EXECUTION),
            (base + 1, 0, 0, EXCEPTION_CONTINUE_SEARCH),
            (base, 8, 0, EXCEPTION_CONTINUE_SEARCH),
            (base, 0, 1, EXCEPTION_CONTINUE_SEARCH),
        ] {
            record.ExceptionInformation[..2].copy_from_slice(&[access, address]);
            record.ExceptionFlags = flags;
            context.Rip = 123;
            // SAFETY: exception references the two live stack records above.
            assert_eq!(unsafe { handler(&mut exception) }, expected);
            let pc = if expected == EXCEPTION_CONTINUE_EXECUTION {
                next as u64
            } else {
                123
            };
            assert_eq!(context.Rip, pc);
        }
    }
}
