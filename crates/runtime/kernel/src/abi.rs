//! Guest calling conventions expressed independently of the host ABI.
use runtime::{CpuAbi, Event, ExecCtx, Guest, GuestFault, TransferAbi, exec::KernelCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convention {
    Stdcall,
    Fastcall,
    Cdecl,
}

/// Arguments are 32-bit ABI words. A by-value u64 occupies two words;
/// the fastcall exports declared here have only register-eligible u32 words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Export {
    pub ordinal: u32,
    pub name: &'static str,
    pub convention: Convention,
    pub words: usize,
    pub variadic: bool,
}
impl Export {
    pub const fn stdcall(ordinal: u32, name: &'static str, words: usize) -> Self {
        Self {
            ordinal,
            name,
            convention: Convention::Stdcall,
            words,
            variadic: false,
        }
    }
    pub const fn fastcall(ordinal: u32, name: &'static str, words: usize) -> Self {
        Self {
            ordinal,
            name,
            convention: Convention::Fastcall,
            words,
            variadic: false,
        }
    }
    pub const fn stack_words(self) -> usize {
        match self.convention {
            Convention::Fastcall => self.words.saturating_sub(2),
            _ => self.words,
        }
    }
    pub const fn cleanup(self) -> u32 {
        match self.convention {
            Convention::Cdecl => 0,
            _ => self.stack_words() as u32 * 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Void,
    U32(u32),
    U64(u64),
}

/// A decoded call has already validated the actual return word and every
/// fixed argument. A fault cannot partially update CPU state or pop a frame.
pub struct Args {
    pub export: Export,
    pub return_pc: u32,
    pub stack: u32,
    words: Vec<u32>,
}
impl Args {
    pub fn read(export: Export, cpu: &CpuAbi, memory: &Guest) -> Result<Self, GuestFault> {
        let return_pc = memory.u32(cpu.esp)?;
        let words = (0..export.words)
            .map(|i| match (export.convention, i) {
                (Convention::Fastcall, 0) => Ok(cpu.ecx),
                (Convention::Fastcall, 1) => Ok(cpu.edx),
                _ => {
                    let stack_i = if export.convention == Convention::Fastcall {
                        i - 2
                    } else {
                        i
                    };
                    memory.u32(cpu.esp.wrapping_add(4 + stack_i as u32 * 4))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            export,
            return_pc,
            stack: cpu.esp,
            words,
        })
    }
    pub fn get(&self, index: usize) -> u32 {
        self.words[index]
    }
    pub fn boolean(&self, index: usize) -> bool {
        self.get(index) as u8 != 0
    }
    pub fn vararg(&self, memory: &Guest, index: usize) -> Result<u32, GuestFault> {
        memory.u32(
            self.stack
                .wrapping_add(4 + (self.export.words + index) as u32 * 4),
        )
    }
    pub fn finish(
        &self,
        cpu: &mut CpuAbi,
        memory: &Guest,
        value: Value,
    ) -> Result<TransferAbi, GuestFault> {
        // Out parameters may alias the saved return word. RET observes the
        // post-service guest stack, never the diagnostic pre-call snapshot.
        let return_pc = memory.u32(self.stack)?;
        match value {
            Value::Void => {}
            Value::U32(v) => cpu.eax = v,
            Value::U64(v) => {
                cpu.eax = v as u32;
                cpu.edx = (v >> 32) as u32;
            }
        }
        cpu.esp = self.stack.wrapping_add(4 + self.export.cleanup());
        Ok(TransferAbi::ret(return_pc))
    }
}

pub(crate) fn stop(export: Export, cpu: &CpuAbi, ctx: &mut ExecCtx) -> TransferAbi {
    let call = {
        let memory = ctx.runtime().memory();
        KernelCall {
            ordinal: export.ordinal,
            name: export.name,
            return_pc: memory.u32(cpu.esp),
            stack_args: (0..export.stack_words())
                .map(|i| memory.u32(cpu.esp.wrapping_add(4 + i as u32 * 4)))
                .collect(),
            variadic: export.variadic,
            cpu: *cpu,
        }
    };
    TransferAbi::stop(ctx.emit(Event::KernelCall(Box::new(call))))
}

/// Register an independent safe export using the same ABI adapter as HLE.
pub fn register<F>(dispatch: &mut runtime::Dispatch, export: Export, f: F)
where
    F: Fn(&Args, &mut Guest) -> Result<Value, GuestFault> + Send + Sync + 'static,
{
    register_at(
        dispatch,
        runtime::kernel::function_token(export.ordinal),
        export,
        f,
    );
}

/// A caller-supplied mapped executable adapter supports ordinals outside
/// Runtime's current loader inventory without pretending they are known tokens.
pub fn register_at<F>(dispatch: &mut runtime::Dispatch, entry: u32, export: Export, f: F)
where
    F: Fn(&Args, &mut Guest) -> Result<Value, GuestFault> + Send + Sync + 'static,
{
    dispatch.register_override(entry, export.name, move |cpu, ctx, _| {
        let mut memory = ctx.runtime().memory();
        let args = Args::read(export, cpu, &memory)?;
        let value = f(&args, &mut memory)?;
        args.finish(cpu, &memory, value)
    });
}
