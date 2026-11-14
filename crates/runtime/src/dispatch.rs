//! Address-to-function table. Generated code calls through this so any guest
//! function can be swapped for a hand-written Rust override without
//! regenerating anything.

use std::collections::HashMap;

use crate::{Cpu, Guest};

pub type GuestFn = fn(&mut Cpu, &mut Guest);

#[derive(Clone, Copy)]
struct Override {
    name: &'static str,
    f: GuestFn,
    enabled: bool,
}

#[derive(Default)]
pub struct Dispatch {
    generated: HashMap<u32, GuestFn>,
    overrides: HashMap<u32, Override>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("no function at {0:#010x}")]
pub struct Missing(pub u32);

impl Dispatch {
    pub fn register_generated(&mut self, addr: u32, f: GuestFn) {
        self.generated.insert(addr, f);
    }

    /// Overrides start enabled; `set_enabled` turns one off to bisect.
    pub fn register_override(&mut self, addr: u32, name: &'static str, f: GuestFn) {
        self.overrides.insert(addr, Override { name, f, enabled: true });
    }

    pub fn set_enabled(&mut self, addr: u32, enabled: bool) -> bool {
        self.overrides.get_mut(&addr).map(|o| o.enabled = enabled).is_some()
    }

    pub fn override_name(&self, addr: u32) -> Option<&'static str> {
        self.overrides.get(&addr).map(|o| o.name)
    }

    pub fn resolve(&self, addr: u32) -> Result<GuestFn, Missing> {
        match self.overrides.get(&addr) {
            Some(o) if o.enabled => Ok(o.f),
            _ => self.generated.get(&addr).copied().ok_or(Missing(addr)),
        }
    }

    pub fn call(&self, addr: u32, cpu: &mut Cpu, guest: &mut Guest) -> Result<(), Missing> {
        self.resolve(addr)?(cpu, guest);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated(cpu: &mut Cpu, _: &mut Guest) {
        cpu.eax = 1;
    }
    fn rewritten(cpu: &mut Cpu, _: &mut Guest) {
        cpu.eax = 2;
    }

    #[test]
    fn override_wins_until_disabled() {
        let mut d = Dispatch::default();
        let (mut cpu, mut g) = (Cpu::default(), Guest::new());
        d.register_generated(0x1234, generated);
        d.call(0x1234, &mut cpu, &mut g).unwrap();
        assert_eq!(cpu.eax, 1);

        d.register_override(0x1234, "rewritten", rewritten);
        d.call(0x1234, &mut cpu, &mut g).unwrap();
        assert_eq!(cpu.eax, 2);

        assert!(d.set_enabled(0x1234, false));
        d.call(0x1234, &mut cpu, &mut g).unwrap();
        assert_eq!(cpu.eax, 1);
    }

    #[test]
    fn missing_address() {
        let d = Dispatch::default();
        assert_eq!(d.resolve(0x10).err(), Some(Missing(0x10)));
    }
}
