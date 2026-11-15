//! Statement builder: allocates temps, folds constants, and keeps flag
//! writes in their architectural place (after every faulting access).

use ir::{BinOp, CastOp, CmpOp, Expr, Flag, FlagValue, Gpr, Slice, Stmt, Temp, UnOp, Value, Width};

/// Builds the statement list for one instruction (or one `Repeat` body).
///
/// Flag writes are collected and appended by [`Builder::finish`], so an
/// instruction's flags only change after all of its loads, stores and fault
/// checks have succeeded. Lowering code can therefore set flags as soon as
/// it has computed them without breaking precise faults.
pub struct Builder {
    stmts: Vec<Stmt>,
    flags: Vec<(Flag, FlagValue)>,
    /// Shared temp counter, so a nested body never reuses an outer id.
    next: u32,
}

impl Builder {
    pub fn new() -> Self {
        Builder { stmts: Vec::new(), flags: Vec::new(), next: 0 }
    }

    /// A builder for a nested statement list (a `Repeat` body) whose temps
    /// continue this one's numbering. Give it back with [`Builder::adopt`].
    pub fn nested(&self) -> Builder {
        Builder { stmts: Vec::new(), flags: Vec::new(), next: self.next }
    }

    /// Take back the temp counter of a nested builder and return its
    /// finished statements.
    pub fn adopt(&mut self, inner: Builder) -> Vec<Stmt> {
        self.next = inner.next;
        inner.finish()
    }

    pub fn push(&mut self, s: Stmt) {
        self.stmts.push(s);
    }

    /// The statements, with pending flag writes last.
    pub fn finish(mut self) -> Vec<Stmt> {
        for (flag, value) in self.flags.drain(..) {
            self.stmts.push(Stmt::WriteFlag { flag, value });
        }
        self.stmts
    }

    pub fn temp(&mut self, width: Width) -> Temp {
        let t = Temp { id: self.next, width };
        self.next += 1;
        t
    }

    /// Assign `expr` to a fresh temp of `width`, or fold it to a constant.
    pub fn assign(&mut self, width: Width, expr: Expr) -> Value {
        if let Some(bits) = expr.fold(width) {
            return Value::constant(width, bits);
        }
        let dst = self.temp(width);
        self.stmts.push(Stmt::Assign { dst, expr });
        dst.into()
    }

    pub fn bin(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        debug_assert_eq!(a.width(), b.width(), "{op:?}");
        // `x op 0 == x` for these: keeps address arithmetic and flag
        // formulas over constants small.
        let identity = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Or | BinOp::Xor | BinOp::Shl | BinOp::Shr | BinOp::Sar);
        if identity && b.as_const() == Some(0) {
            return a;
        }
        self.assign(a.width(), Expr::Binary { op, a, b })
    }

    pub fn un(&mut self, op: UnOp, a: Value) -> Value {
        let w = if op == UnOp::Parity { Width::W1 } else { a.width() };
        self.assign(w, Expr::Unary { op, a })
    }

    pub fn cmp(&mut self, op: CmpOp, a: Value, b: Value) -> Value {
        self.assign(Width::W1, Expr::Compare { op, a, b })
    }

    pub fn cast(&mut self, op: CastOp, a: Value, to: Width) -> Value {
        if a.width() == to {
            return a;
        }
        self.assign(to, Expr::Cast { op, a })
    }

    pub fn zext(&mut self, a: Value, to: Width) -> Value {
        self.cast(CastOp::Zext, a, to)
    }

    pub fn sext(&mut self, a: Value, to: Width) -> Value {
        self.cast(CastOp::Sext, a, to)
    }

    pub fn trunc(&mut self, a: Value, to: Width) -> Value {
        self.cast(CastOp::Trunc, a, to)
    }

    pub fn select(&mut self, cond: Value, a: Value, b: Value) -> Value {
        match cond.as_const() {
            Some(1) => return a,
            Some(0) => return b,
            _ => {}
        }
        if a == b {
            return a;
        }
        self.assign(a.width(), Expr::Select { cond, a, b })
    }

    pub fn and(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::And, a, b)
    }

    pub fn or(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Or, a, b)
    }

    pub fn xor(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Xor, a, b)
    }

    pub fn not(&mut self, a: Value) -> Value {
        self.un(UnOp::Not, a)
    }

    pub fn add(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Add, a, b)
    }

    pub fn sub(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Sub, a, b)
    }

    /// Bit `n` of `a` as a `W1`.
    pub fn bit(&mut self, a: Value, n: u32) -> Value {
        let w = a.width();
        let s = self.bin(BinOp::Shr, a, Value::constant(w, n as u128));
        self.trunc(s, Width::W1)
    }

    /// The most significant bit of `a`.
    pub fn msb(&mut self, a: Value) -> Value {
        let n = a.width().bits() - 1;
        self.bit(a, n)
    }

    pub fn is_zero(&mut self, a: Value) -> Value {
        let z = Value::constant(a.width(), 0);
        self.cmp(CmpOp::Eq, a, z)
    }

    pub fn read_reg(&mut self, reg: Gpr, slice: Slice) -> Value {
        let dst = self.temp(slice.width());
        self.stmts.push(Stmt::ReadReg { dst, reg, slice });
        dst.into()
    }

    pub fn write_reg(&mut self, reg: Gpr, slice: Slice, value: Value) {
        self.stmts.push(Stmt::WriteReg { reg, slice, value });
    }

    pub fn read_flag(&mut self, flag: Flag) -> Value {
        let dst = self.temp(Width::W1);
        self.stmts.push(Stmt::ReadFlag { dst, flag });
        dst.into()
    }

    /// Queue a flag write; it is emitted after every other statement.
    pub fn set_flag(&mut self, flag: Flag, value: Value) {
        self.queue_flag(flag, FlagValue::Defined(value));
    }

    pub fn undef_flag(&mut self, flag: Flag) {
        self.queue_flag(flag, FlagValue::Undefined);
    }

    fn queue_flag(&mut self, flag: Flag, value: FlagValue) {
        // One write per flag: a later definition replaces an earlier one.
        self.flags.retain(|(f, _)| *f != flag);
        self.flags.push((flag, value));
    }

    pub fn load(&mut self, addr: ir::Addr, width: Width) -> Value {
        let dst = self.temp(width);
        self.stmts.push(Stmt::Load { dst, addr });
        dst.into()
    }

    pub fn store(&mut self, addr: ir::Addr, value: Value) {
        self.stmts.push(Stmt::Store { addr, value });
    }

    pub fn fault_if(&mut self, cond: Value, fault: ir::Fault) {
        if cond.as_const() == Some(0) {
            return;
        }
        self.stmts.push(Stmt::FaultIf { cond, fault });
    }
}
