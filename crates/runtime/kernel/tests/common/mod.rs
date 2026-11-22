#![allow(dead_code)]
use kernel::{
    Config, Kernel,
    abi::{Convention, Export},
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestFault, Runner, Runtime, TransferAbi,
    guest::Permissions,
};
use std::sync::Arc;
pub const DATA: u32 = 0x10000;
pub const CODE: u32 = 0x400000;
pub const STACK_BOTTOM: u32 = 0x6fff0000;
pub const STACK_TOP: u32 = 0x70000000;

pub struct Harness {
    pub runtime: Arc<Runtime>,
    pub kernel: Arc<Kernel>,
    pub cpu: CpuAbi,
    pub ctx: ExecCtx,
}
impl Harness {
    pub fn new() -> Self {
        Self::config(Config::default())
    }
    pub fn config(config: Config) -> Self {
        Self::with_guest(config, |_| {})
    }
    /// `setup` runs on the guest (DATA, CODE and the stack already mapped)
    /// before the kernel installs, e.g. to add physical reservations.
    pub fn with_guest(config: Config, setup: impl FnOnce(&mut Guest)) -> Self {
        Self::with_memory(config, Guest::new(), setup)
    }

    pub fn with_memory(config: Config, mut memory: Guest, setup: impl FnOnce(&mut Guest)) -> Self {
        memory.map_zeroed(DATA, 0x20000, Permissions::RW).unwrap();
        memory.map_zeroed(CODE, 0x10000, Permissions::RWX).unwrap();
        memory
            .map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)
            .unwrap();
        setup(&mut memory);
        let runtime = Runtime::new(memory, Dispatch::default());
        let kernel = Kernel::install(&runtime, config).unwrap();
        let cpu = CpuAbi {
            eip: CODE,
            esp: STACK_TOP - 4,
            ..CpuAbi::xbox_thread()
        };
        runtime
            .memory()
            .set_u32(cpu.esp, runtime::exec::HARNESS_EXIT)
            .unwrap();
        let ctx = ExecCtx::new(Arc::clone(&runtime), 1);
        Self {
            runtime,
            kernel,
            cpu,
            ctx,
        }
    }
    pub fn set(&self, address: u32, value: u32) {
        self.runtime.memory().set_u32(address, value).unwrap();
    }
    pub fn get(&self, address: u32) -> u32 {
        self.runtime.memory().u32(address).unwrap()
    }
    pub fn bytes(&self, address: u32, bytes: &[u8]) {
        self.runtime
            .memory()
            .write_bytes(address, bytes, 1)
            .unwrap();
    }
    pub fn call(&mut self, ordinal: u32, args: &[u32]) -> TransferAbi {
        let export = *kernel::EXPORTS
            .iter()
            .find(|e| e.ordinal == ordinal)
            .unwrap();
        self.cpu.esp = STACK_TOP - 0x100;
        self.cpu.eax = 0xfeedface;
        self.cpu.edx = 0x12345678;
        frame(
            &self.runtime,
            &mut self.cpu,
            export,
            args,
            runtime::exec::HARNESS_EXIT,
        );
        let entry = self.kernel.entry(ordinal).unwrap();
        Runner { max_transfers: 10 }.run(&mut self.cpu, &mut self.ctx, entry)
    }
    pub fn success(&mut self, ordinal: u32, args: &[u32]) -> u32 {
        let transfer = self.call(ordinal, args);
        assert!(
            matches!(self.ctx.event(transfer.event_id), Some(Event::HarnessExit)),
            "ordinal {ordinal}: {:?}, {:?}",
            transfer,
            self.ctx.event(transfer.event_id)
        );
        let export = *kernel::EXPORTS
            .iter()
            .find(|e| e.ordinal == ordinal)
            .unwrap();
        assert_eq!(
            self.cpu.esp,
            STACK_TOP - 0x100 + 4 + export.cleanup(),
            "ordinal {ordinal} cleanup"
        );
        self.cpu.eax
    }
    /// Fill the guest handle table to `limits::MAX_HANDLES` with an event
    /// and its duplicates. Returns one duplicate, which the caller may close
    /// to free exactly one slot. Uses the last 16 bytes of DATA as scratch.
    pub fn fill_handles(&mut self) -> u32 {
        const OUT: u32 = DATA + 0x1fff0;
        assert_eq!(self.success(189, &[OUT, 0, 0, 0]), 0);
        let event = self.get(OUT);
        let mut last = 0;
        for i in 1.. {
            assert!(
                i <= kernel::limits::MAX_HANDLES,
                "handle table never filled"
            );
            if i % 100 == 0 {
                // Keep the per-context event log small.
                self.ctx = ExecCtx::new(Arc::clone(&self.runtime), 1);
            }
            match self.success(197, &[event, OUT + 4, 0]) {
                0 => last = self.get(OUT + 4),
                kernel::status::INSUFFICIENT_RESOURCES => break,
                other => panic!("NtDuplicateObject: {other:#x}"),
            }
        }
        assert_ne!(last, 0);
        last
    }
    pub fn stopped(&mut self, ordinal: u32, args: &[u32]) {
        let transfer = self.call(ordinal, args);
        assert_eq!(transfer.kind, runtime::abi::STOP);
        assert!(
            matches!(self.ctx.event(transfer.event_id), Some(Event::KernelCall(call)) if call.ordinal == ordinal)
        );
        assert_eq!(self.cpu.esp, STACK_TOP - 0x100);
        assert_eq!(self.cpu.eax, 0xfeedface);
    }
    pub fn code<F>(&self, offset: u32, f: F)
    where
        F: Fn(&mut CpuAbi, &mut ExecCtx, u32) -> Result<TransferAbi, GuestFault>
            + Send
            + Sync
            + 'static,
    {
        self.runtime.edit_dispatch(|d| {
            d.register_override(CODE + offset, "synthetic guest", f);
        });
    }
}
pub fn frame(runtime: &Runtime, cpu: &mut CpuAbi, export: Export, args: &[u32], return_pc: u32) {
    assert!(args.len() >= export.words);
    let mut memory = runtime.memory();
    memory.set_u32(cpu.esp, return_pc).unwrap();
    for (i, &word) in args.iter().enumerate() {
        match (export.convention, i) {
            (Convention::Fastcall, 0) => cpu.ecx = word,
            (Convention::Fastcall, 1) => cpu.edx = word,
            _ => {
                let index = if export.convention == Convention::Fastcall {
                    i - 2
                } else {
                    i
                };
                memory
                    .set_u32(cpu.esp + 4 + index as u32 * 4, word)
                    .unwrap();
            }
        }
    }
}
pub fn guest_call(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    entry: u32,
    args: &[u32],
    continuation: u32,
) -> Result<TransferAbi, GuestFault> {
    let mut memory = ctx.memory();
    let esp = cpu.esp - args.len() as u32 * 4;
    for (i, word) in args.iter().enumerate() {
        memory.set_u32(esp + i as u32 * 4, *word)?;
    }
    cpu.esp = esp;
    memory.call(cpu, entry, continuation)
}
