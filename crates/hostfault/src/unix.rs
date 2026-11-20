//! The signal handler and its chaining (moved from the runtime's flat RAM
//! write watch, `crates/runtime/src/flat/protection.rs`, which F2 deletes).
use super::{Error, Fault, dispatch};
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::OnceLock;

#[cfg(target_vendor = "apple")]
const SIGNALS: [i32; 2] = [libc::SIGBUS, libc::SIGSEGV];
#[cfg(not(target_vendor = "apple"))]
const SIGNALS: [i32; 1] = [libc::SIGSEGV];

/// The action each of [`SIGNALS`] had before ours, published before ours
/// is installed and never changed.
static PREVIOUS: [OnceLock<libc::sigaction>; 2] = [OnceLock::new(), OnceLock::new()];

pub(super) fn install() -> Result<(), Error> {
    static INSTALLED: OnceLock<Result<(), Error>> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        for (i, &sig) in SIGNALS.iter().enumerate() {
            // SAFETY: plain sigaction calls on a zeroed, then filled, struct.
            unsafe {
                let mut action = MaybeUninit::zeroed();
                if libc::sigaction(sig, ptr::null(), action.as_mut_ptr()) != 0 {
                    return Err(Error::Install);
                }
                let mut action: libc::sigaction = action.assume_init();
                // An automatically reset action cannot be chained persistently.
                if action.sa_flags & libc::SA_RESETHAND != 0 {
                    return Err(Error::Install);
                }
                if PREVIOUS[i].set(action).is_err() {
                    return Err(Error::Install);
                }
                action.sa_sigaction = handler as *const () as usize;
                // Keep the previous mask and restart behaviour; run on the
                // alternate stack where the thread has one, so a host stack
                // overflow still reaches Rust's report.
                action.sa_flags |= libc::SA_SIGINFO | libc::SA_ONSTACK;
                if libc::sigaction(sig, &action, ptr::null_mut()) != 0 {
                    return Err(Error::Install);
                }
            }
        }
        Ok(())
    })
}

/// The saved PC of the interrupted thread.
///
/// # Safety
/// `context` is the `ucontext_t` a three-argument handler was given.
unsafe fn pc(context: *mut libc::c_void) -> usize {
    let uc = context.cast::<libc::ucontext_t>();
    // SAFETY: per the contract above.
    unsafe {
        #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
        return (*(*uc).uc_mcontext).__ss.__pc as usize;
        #[cfg(all(target_vendor = "apple", target_arch = "x86_64"))]
        return (*(*uc).uc_mcontext).__ss.__rip as usize;
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        return (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] as usize;
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        return (*uc).uc_mcontext.pc as usize;
    }
}

/// Make the interrupted thread resume at `pc`.
///
/// # Safety
/// As [`pc`].
pub(super) unsafe fn set_pc(context: *mut libc::c_void, pc: usize) {
    let uc = context.cast::<libc::ucontext_t>();
    // SAFETY: per the contract above.
    unsafe {
        #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
        {
            (*(*uc).uc_mcontext).__ss.__pc = pc as u64;
        }
        #[cfg(all(target_vendor = "apple", target_arch = "x86_64"))]
        {
            (*(*uc).uc_mcontext).__ss.__rip = pc as u64;
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] = pc as i64;
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            (*uc).uc_mcontext.pc = pc as u64;
        }
    }
}

extern "C" fn handler(sig: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // Only faults the hardware raised (SEGV_MAPERR/ACCERR, BUS_ADRALN/
    // ADRERR/OBJERR; Darwin reports a protection fault as SIGBUS with
    // BUS_ADRALN), never a kill() or raise() of the same signal.
    // SAFETY: the kernel passes a valid siginfo and context to SA_SIGINFO.
    let code = unsafe { (*info).si_code };
    let hardware = (1..=3).contains(&code);
    if hardware {
        // SAFETY: as above.
        let (address, pc) = unsafe { ((*info).si_addr() as usize, pc(context)) };
        let mut fault = Fault {
            address,
            pc,
            context,
        };
        if dispatch(&mut fault) {
            return;
        }
    }
    forward(sig, info, context, hardware);
}

/// Hand the signal, siginfo and mutable saved context to the action that
/// was installed before ours.
fn forward(sig: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void, hardware: bool) {
    let index = SIGNALS.iter().position(|&s| s == sig);
    let Some(old) = index.and_then(|i| PREVIOUS[i].get()) else {
        // SAFETY: async-signal-safe.
        unsafe { libc::_exit(128 + sig) }
    };
    // SAFETY: calling the previous action as the kernel would have.
    unsafe {
        match old.sa_sigaction {
            libc::SIG_IGN => {}
            libc::SIG_DFL => {
                // Restore the default. A hardware fault then returns and
                // faults again, so the default action (a core, or the
                // debugger) sees the original instruction; a sent signal
                // is raised again, delivered once this handler returns.
                if libc::sigaction(sig, old, ptr::null_mut()) != 0
                    || (!hardware && libc::raise(sig) != 0)
                {
                    libc::_exit(128 + sig);
                }
            }
            f if old.sa_flags & libc::SA_SIGINFO != 0 => {
                std::mem::transmute::<
                    usize,
                    extern "C" fn(i32, *mut libc::siginfo_t, *mut libc::c_void),
                >(f)(sig, info, context);
            }
            f => std::mem::transmute::<usize, extern "C" fn(i32)>(f)(sig),
        }
    }
}
