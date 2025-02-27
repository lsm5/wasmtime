//! AArch64 ISA definitions: instruction arguments.

use crate::ir::types::*;
use crate::ir::Type;
use crate::isa::aarch64::inst::*;
use crate::machinst::{ty_bits, MachLabel, PrettyPrint, Reg, Writable};
use core::convert::Into;
use std::string::String;

//=============================================================================
// Instruction sub-components: shift and extend descriptors

/// A shift operator for a register or immediate.
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum ShiftOp {
    LSL = 0b00,
    #[allow(dead_code)]
    LSR = 0b01,
    #[allow(dead_code)]
    ASR = 0b10,
    #[allow(dead_code)]
    ROR = 0b11,
}

impl ShiftOp {
    /// Get the encoding of this shift op.
    pub fn bits(self) -> u8 {
        self as u8
    }
}

/// A shift operator amount.
#[derive(Clone, Copy, Debug)]
pub struct ShiftOpShiftImm(u8);

impl ShiftOpShiftImm {
    /// Maximum shift for shifted-register operands.
    pub const MAX_SHIFT: u64 = 63;

    /// Create a new shiftop shift amount, if possible.
    pub fn maybe_from_shift(shift: u64) -> Option<ShiftOpShiftImm> {
        if shift <= Self::MAX_SHIFT {
            Some(ShiftOpShiftImm(shift as u8))
        } else {
            None
        }
    }

    /// Return the shift amount.
    pub fn value(self) -> u8 {
        self.0
    }

    /// Mask down to a given number of bits.
    pub fn mask(self, bits: u8) -> ShiftOpShiftImm {
        ShiftOpShiftImm(self.0 & (bits - 1))
    }
}

/// A shift operator with an amount, guaranteed to be within range.
#[derive(Copy, Clone, Debug)]
pub struct ShiftOpAndAmt {
    op: ShiftOp,
    shift: ShiftOpShiftImm,
}

impl ShiftOpAndAmt {
    pub fn new(op: ShiftOp, shift: ShiftOpShiftImm) -> ShiftOpAndAmt {
        ShiftOpAndAmt { op, shift }
    }

    /// Get the shift op.
    pub fn op(&self) -> ShiftOp {
        self.op
    }

    /// Get the shift amount.
    pub fn amt(&self) -> ShiftOpShiftImm {
        self.shift
    }
}

/// An extend operator for a register.
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum ExtendOp {
    UXTB = 0b000,
    UXTH = 0b001,
    UXTW = 0b010,
    UXTX = 0b011,
    SXTB = 0b100,
    SXTH = 0b101,
    SXTW = 0b110,
    #[allow(dead_code)]
    SXTX = 0b111,
}

impl ExtendOp {
    /// Encoding of this op.
    pub fn bits(self) -> u8 {
        self as u8
    }
}

//=============================================================================
// Instruction sub-components (memory addresses): definitions

/// A reference to some memory address.
#[derive(Clone, Debug)]
pub enum MemLabel {
    /// An address in the code, a constant pool or jumptable, with relative
    /// offset from this instruction. This form must be used at emission time;
    /// see `memlabel_finalize()` for how other forms are lowered to this one.
    PCRel(i32),
}

/// An addressing mode specified for a load/store operation.
#[derive(Clone, Debug)]
pub enum AMode {
    //
    // Real ARM64 addressing modes:
    //
    /// "post-indexed" mode as per AArch64 docs: postincrement reg after address computation.
    PostIndexed(Writable<Reg>, SImm9),
    /// "pre-indexed" mode as per AArch64 docs: preincrement reg before address computation.
    PreIndexed(Writable<Reg>, SImm9),

    // N.B.: RegReg, RegScaled, and RegScaledExtended all correspond to
    // what the ISA calls the "register offset" addressing mode. We split out
    // several options here for more ergonomic codegen.
    /// Register plus register offset.
    RegReg(Reg, Reg),

    #[allow(dead_code)]
    /// Register plus register offset, scaled by type's size.
    RegScaled(Reg, Reg, Type),

    /// Register plus register offset, scaled by type's size, with index sign- or zero-extended
    /// first.
    RegScaledExtended(Reg, Reg, Type, ExtendOp),

    /// Register plus register offset, with index sign- or zero-extended first.
    RegExtended(Reg, Reg, ExtendOp),

    /// Unscaled signed 9-bit immediate offset from reg.
    Unscaled(Reg, SImm9),

    /// Scaled (by size of a type) unsigned 12-bit immediate offset from reg.
    UnsignedOffset(Reg, UImm12Scaled),

    //
    // virtual addressing modes that are lowered at emission time:
    //
    /// Reference to a "label": e.g., a symbol.
    Label(MemLabel),

    /// Arbitrary offset from a register. Converted to generation of large
    /// offsets with multiple instructions as necessary during code emission.
    RegOffset(Reg, i64, Type),

    /// Offset from the stack pointer.
    SPOffset(i64, Type),

    /// Offset from the frame pointer.
    FPOffset(i64, Type),

    /// Offset from the "nominal stack pointer", which is where the real SP is
    /// just after stack and spill slots are allocated in the function prologue.
    /// At emission time, this is converted to `SPOffset` with a fixup added to
    /// the offset constant. The fixup is a running value that is tracked as
    /// emission iterates through instructions in linear order, and can be
    /// adjusted up and down with [Inst::VirtualSPOffsetAdj].
    ///
    /// The standard ABI is in charge of handling this (by emitting the
    /// adjustment meta-instructions). It maintains the invariant that "nominal
    /// SP" is where the actual SP is after the function prologue and before
    /// clobber pushes. See the diagram in the documentation for
    /// [crate::isa::aarch64::abi](the ABI module) for more details.
    NominalSPOffset(i64, Type),
}

impl AMode {
    /// Memory reference using an address in a register.
    pub fn reg(reg: Reg) -> AMode {
        // Use UnsignedOffset rather than Unscaled to use ldr rather than ldur.
        // This also does not use PostIndexed / PreIndexed as they update the register.
        AMode::UnsignedOffset(reg, UImm12Scaled::zero(I64))
    }

    /// Memory reference using `reg1 + sizeof(ty) * reg2` as an address, with `reg2` sign- or
    /// zero-extended as per `op`.
    pub fn reg_plus_reg_scaled_extended(reg1: Reg, reg2: Reg, ty: Type, op: ExtendOp) -> AMode {
        AMode::RegScaledExtended(reg1, reg2, ty, op)
    }

    /// Does the address resolve to just a register value, with no offset or
    /// other computation?
    pub fn is_reg(&self) -> Option<Reg> {
        match self {
            &AMode::UnsignedOffset(r, uimm12) if uimm12.value() == 0 => Some(r),
            &AMode::Unscaled(r, imm9) if imm9.value() == 0 => Some(r),
            &AMode::RegOffset(r, off, _) if off == 0 => Some(r),
            &AMode::FPOffset(off, _) if off == 0 => Some(fp_reg()),
            &AMode::SPOffset(off, _) if off == 0 => Some(stack_reg()),
            _ => None,
        }
    }

    pub fn with_allocs(&self, allocs: &mut AllocationConsumer<'_>) -> Self {
        // This should match `memarg_operands()`.
        match self {
            &AMode::Unscaled(reg, imm9) => AMode::Unscaled(allocs.next(reg), imm9),
            &AMode::UnsignedOffset(r, uimm12) => AMode::UnsignedOffset(allocs.next(r), uimm12),
            &AMode::RegReg(r1, r2) => AMode::RegReg(allocs.next(r1), allocs.next(r2)),
            &AMode::RegScaled(r1, r2, ty) => AMode::RegScaled(allocs.next(r1), allocs.next(r2), ty),
            &AMode::RegScaledExtended(r1, r2, ty, ext) => {
                AMode::RegScaledExtended(allocs.next(r1), allocs.next(r2), ty, ext)
            }
            &AMode::RegExtended(r1, r2, ext) => {
                AMode::RegExtended(allocs.next(r1), allocs.next(r2), ext)
            }
            &AMode::PreIndexed(reg, simm9) => AMode::PreIndexed(allocs.next_writable(reg), simm9),
            &AMode::PostIndexed(reg, simm9) => AMode::PostIndexed(allocs.next_writable(reg), simm9),
            &AMode::RegOffset(r, off, ty) => AMode::RegOffset(allocs.next(r), off, ty),
            &AMode::FPOffset(..)
            | &AMode::SPOffset(..)
            | &AMode::NominalSPOffset(..)
            | AMode::Label(..) => self.clone(),
        }
    }
}

/// A memory argument to a load/store-pair.
#[derive(Clone, Debug)]
pub enum PairAMode {
    SignedOffset(Reg, SImm7Scaled),
    PreIndexed(Writable<Reg>, SImm7Scaled),
    PostIndexed(Writable<Reg>, SImm7Scaled),
}

impl PairAMode {
    pub fn with_allocs(&self, allocs: &mut AllocationConsumer<'_>) -> Self {
        // Should match `pairmemarg_operands()`.
        match self {
            &PairAMode::SignedOffset(reg, simm7scaled) => {
                PairAMode::SignedOffset(allocs.next(reg), simm7scaled)
            }
            &PairAMode::PreIndexed(reg, simm7scaled) => {
                PairAMode::PreIndexed(allocs.next_writable(reg), simm7scaled)
            }
            &PairAMode::PostIndexed(reg, simm7scaled) => {
                PairAMode::PostIndexed(allocs.next_writable(reg), simm7scaled)
            }
        }
    }
}

//=============================================================================
// Instruction sub-components (conditions, branches and branch targets):
// definitions

/// Condition for conditional branches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cond {
    Eq = 0,
    Ne = 1,
    Hs = 2,
    Lo = 3,
    Mi = 4,
    Pl = 5,
    Vs = 6,
    Vc = 7,
    Hi = 8,
    Ls = 9,
    Ge = 10,
    Lt = 11,
    Gt = 12,
    Le = 13,
    Al = 14,
    Nv = 15,
}

impl Cond {
    /// Return the inverted condition.
    pub fn invert(self) -> Cond {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,

            Cond::Hs => Cond::Lo,
            Cond::Lo => Cond::Hs,

            Cond::Mi => Cond::Pl,
            Cond::Pl => Cond::Mi,

            Cond::Vs => Cond::Vc,
            Cond::Vc => Cond::Vs,

            Cond::Hi => Cond::Ls,
            Cond::Ls => Cond::Hi,

            Cond::Ge => Cond::Lt,
            Cond::Lt => Cond::Ge,

            Cond::Gt => Cond::Le,
            Cond::Le => Cond::Gt,

            Cond::Al => Cond::Nv,
            Cond::Nv => Cond::Al,
        }
    }

    /// Return the machine encoding of this condition.
    pub fn bits(self) -> u32 {
        self as u32
    }
}

/// The kind of conditional branch: the common-case-optimized "reg-is-zero" /
/// "reg-is-nonzero" variants, or the generic one that tests the machine
/// condition codes.
#[derive(Clone, Copy, Debug)]
pub enum CondBrKind {
    /// Condition: given register is zero.
    Zero(Reg),
    /// Condition: given register is nonzero.
    NotZero(Reg),
    /// Condition: the given condition-code test is true.
    Cond(Cond),
}

impl CondBrKind {
    /// Return the inverted branch condition.
    pub fn invert(self) -> CondBrKind {
        match self {
            CondBrKind::Zero(reg) => CondBrKind::NotZero(reg),
            CondBrKind::NotZero(reg) => CondBrKind::Zero(reg),
            CondBrKind::Cond(c) => CondBrKind::Cond(c.invert()),
        }
    }
}

/// A branch target. Either unresolved (basic-block index) or resolved (offset
/// from end of current instruction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchTarget {
    /// An unresolved reference to a Label, as passed into
    /// `lower_branch_group()`.
    Label(MachLabel),
    /// A fixed PC offset.
    ResolvedOffset(i32),
}

impl BranchTarget {
    /// Return the target's label, if it is a label-based target.
    pub fn as_label(self) -> Option<MachLabel> {
        match self {
            BranchTarget::Label(l) => Some(l),
            _ => None,
        }
    }

    /// Return the target's offset, if specified, or zero if label-based.
    pub fn as_offset19_or_zero(self) -> u32 {
        let off = match self {
            BranchTarget::ResolvedOffset(off) => off >> 2,
            _ => 0,
        };
        assert!(off <= 0x3ffff);
        assert!(off >= -0x40000);
        (off as u32) & 0x7ffff
    }

    /// Return the target's offset, if specified, or zero if label-based.
    pub fn as_offset26_or_zero(self) -> u32 {
        let off = match self {
            BranchTarget::ResolvedOffset(off) => off >> 2,
            _ => 0,
        };
        assert!(off <= 0x1ffffff);
        assert!(off >= -0x2000000);
        (off as u32) & 0x3ffffff
    }
}

impl PrettyPrint for ShiftOpAndAmt {
    fn pretty_print(&self, _: u8, _: &mut AllocationConsumer<'_>) -> String {
        format!("{:?} {}", self.op(), self.amt().value())
    }
}

impl PrettyPrint for ExtendOp {
    fn pretty_print(&self, _: u8, _: &mut AllocationConsumer<'_>) -> String {
        format!("{:?}", self)
    }
}

impl PrettyPrint for MemLabel {
    fn pretty_print(&self, _: u8, _: &mut AllocationConsumer<'_>) -> String {
        match self {
            &MemLabel::PCRel(off) => format!("pc+{}", off),
        }
    }
}

fn shift_for_type(ty: Type) -> usize {
    match ty.bytes() {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        16 => 4,
        _ => panic!("unknown type: {}", ty),
    }
}

impl PrettyPrint for AMode {
    fn pretty_print(&self, _: u8, allocs: &mut AllocationConsumer<'_>) -> String {
        match self {
            &AMode::Unscaled(reg, simm9) => {
                let reg = pretty_print_reg(reg, allocs);
                if simm9.value != 0 {
                    let simm9 = simm9.pretty_print(8, allocs);
                    format!("[{}, {}]", reg, simm9)
                } else {
                    format!("[{}]", reg)
                }
            }
            &AMode::UnsignedOffset(reg, uimm12) => {
                let reg = pretty_print_reg(reg, allocs);
                if uimm12.value != 0 {
                    let uimm12 = uimm12.pretty_print(8, allocs);
                    format!("[{}, {}]", reg, uimm12)
                } else {
                    format!("[{}]", reg)
                }
            }
            &AMode::RegReg(r1, r2) => {
                let r1 = pretty_print_reg(r1, allocs);
                let r2 = pretty_print_reg(r2, allocs);
                format!("[{}, {}]", r1, r2)
            }
            &AMode::RegScaled(r1, r2, ty) => {
                let r1 = pretty_print_reg(r1, allocs);
                let r2 = pretty_print_reg(r2, allocs);
                let shift = shift_for_type(ty);
                format!("[{}, {}, LSL #{}]", r1, r2, shift)
            }
            &AMode::RegScaledExtended(r1, r2, ty, op) => {
                let shift = shift_for_type(ty);
                let size = match op {
                    ExtendOp::SXTW | ExtendOp::UXTW => OperandSize::Size32,
                    _ => OperandSize::Size64,
                };
                let r1 = pretty_print_reg(r1, allocs);
                let r2 = pretty_print_ireg(r2, size, allocs);
                let op = op.pretty_print(0, allocs);
                format!("[{}, {}, {} #{}]", r1, r2, op, shift)
            }
            &AMode::RegExtended(r1, r2, op) => {
                let size = match op {
                    ExtendOp::SXTW | ExtendOp::UXTW => OperandSize::Size32,
                    _ => OperandSize::Size64,
                };
                let r1 = pretty_print_reg(r1, allocs);
                let r2 = pretty_print_ireg(r2, size, allocs);
                let op = op.pretty_print(0, allocs);
                format!("[{}, {}, {}]", r1, r2, op)
            }
            &AMode::Label(ref label) => label.pretty_print(0, allocs),
            &AMode::PreIndexed(r, simm9) => {
                let r = pretty_print_reg(r.to_reg(), allocs);
                let simm9 = simm9.pretty_print(8, allocs);
                format!("[{}, {}]!", r, simm9)
            }
            &AMode::PostIndexed(r, simm9) => {
                let r = pretty_print_reg(r.to_reg(), allocs);
                let simm9 = simm9.pretty_print(8, allocs);
                format!("[{}], {}", r, simm9)
            }
            // Eliminated by `mem_finalize()`.
            &AMode::SPOffset(..)
            | &AMode::FPOffset(..)
            | &AMode::NominalSPOffset(..)
            | &AMode::RegOffset(..) => {
                panic!("Unexpected pseudo mem-arg mode (stack-offset or generic reg-offset)!")
            }
        }
    }
}

impl PrettyPrint for PairAMode {
    fn pretty_print(&self, _: u8, allocs: &mut AllocationConsumer<'_>) -> String {
        match self {
            &PairAMode::SignedOffset(reg, simm7) => {
                let reg = pretty_print_reg(reg, allocs);
                if simm7.value != 0 {
                    let simm7 = simm7.pretty_print(8, allocs);
                    format!("[{}, {}]", reg, simm7)
                } else {
                    format!("[{}]", reg)
                }
            }
            &PairAMode::PreIndexed(reg, simm7) => {
                let reg = pretty_print_reg(reg.to_reg(), allocs);
                let simm7 = simm7.pretty_print(8, allocs);
                format!("[{}, {}]!", reg, simm7)
            }
            &PairAMode::PostIndexed(reg, simm7) => {
                let reg = pretty_print_reg(reg.to_reg(), allocs);
                let simm7 = simm7.pretty_print(8, allocs);
                format!("[{}], {}", reg, simm7)
            }
        }
    }
}

impl PrettyPrint for Cond {
    fn pretty_print(&self, _: u8, _: &mut AllocationConsumer<'_>) -> String {
        let mut s = format!("{:?}", self);
        s.make_ascii_lowercase();
        s
    }
}

impl PrettyPrint for BranchTarget {
    fn pretty_print(&self, _: u8, _: &mut AllocationConsumer<'_>) -> String {
        match self {
            &BranchTarget::Label(label) => format!("label{:?}", label.get()),
            &BranchTarget::ResolvedOffset(off) => format!("{}", off),
        }
    }
}

/// Type used to communicate the operand size of a machine instruction, as AArch64 has 32- and
/// 64-bit variants of many instructions (and integer registers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandSize {
    Size32,
    Size64,
}

impl OperandSize {
    /// 32-bit case?
    pub fn is32(self) -> bool {
        self == OperandSize::Size32
    }

    /// 64-bit case?
    pub fn is64(self) -> bool {
        self == OperandSize::Size64
    }

    /// Convert from a needed width to the smallest size that fits.
    pub fn from_bits<I: Into<usize>>(bits: I) -> OperandSize {
        let bits: usize = bits.into();
        assert!(bits <= 64);
        if bits <= 32 {
            OperandSize::Size32
        } else {
            OperandSize::Size64
        }
    }

    pub fn bits(&self) -> u8 {
        match self {
            OperandSize::Size32 => 32,
            OperandSize::Size64 => 64,
        }
    }

    /// Convert from an integer type into the smallest size that fits.
    pub fn from_ty(ty: Type) -> OperandSize {
        debug_assert!(!ty.is_vector());

        Self::from_bits(ty_bits(ty))
    }

    /// Convert to I32, I64, or I128.
    pub fn to_ty(self) -> Type {
        match self {
            OperandSize::Size32 => I32,
            OperandSize::Size64 => I64,
        }
    }

    pub fn sf_bit(&self) -> u32 {
        match self {
            OperandSize::Size32 => 0,
            OperandSize::Size64 => 1,
        }
    }
}

/// Type used to communicate the size of a scalar SIMD & FP operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarSize {
    Size8,
    Size16,
    Size32,
    Size64,
    Size128,
}

impl ScalarSize {
    /// Convert from a needed width to the smallest size that fits.
    pub fn from_bits<I: Into<usize>>(bits: I) -> ScalarSize {
        match bits.into().next_power_of_two() {
            8 => ScalarSize::Size8,
            16 => ScalarSize::Size16,
            32 => ScalarSize::Size32,
            64 => ScalarSize::Size64,
            128 => ScalarSize::Size128,
            w => panic!("Unexpected type width: {}", w),
        }
    }

    /// Convert to an integer operand size.
    pub fn operand_size(&self) -> OperandSize {
        match self {
            ScalarSize::Size32 => OperandSize::Size32,
            ScalarSize::Size64 => OperandSize::Size64,
            _ => panic!("Unexpected operand_size request for: {:?}", self),
        }
    }

    /// Convert from a type into the smallest size that fits.
    pub fn from_ty(ty: Type) -> ScalarSize {
        debug_assert!(!ty.is_vector());

        Self::from_bits(ty_bits(ty))
    }

    /// Return the encoding bits that are used by some scalar FP instructions
    /// for a particular operand size.
    pub fn ftype(&self) -> u32 {
        match self {
            ScalarSize::Size16 => 0b11,
            ScalarSize::Size32 => 0b00,
            ScalarSize::Size64 => 0b01,
            _ => panic!("Unexpected scalar FP operand size: {:?}", self),
        }
    }

    pub fn widen(&self) -> ScalarSize {
        match self {
            ScalarSize::Size8 => ScalarSize::Size16,
            ScalarSize::Size16 => ScalarSize::Size32,
            ScalarSize::Size32 => ScalarSize::Size64,
            ScalarSize::Size64 => ScalarSize::Size128,
            ScalarSize::Size128 => panic!("can't widen 128-bits"),
        }
    }
}

/// Type used to communicate the size of a vector operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorSize {
    Size8x8,
    Size8x16,
    Size16x4,
    Size16x8,
    Size32x2,
    Size32x4,
    Size64x2,
}

impl VectorSize {
    /// Get the vector operand size with the given scalar size as lane size.
    pub fn from_lane_size(size: ScalarSize, is_128bit: bool) -> VectorSize {
        match (size, is_128bit) {
            (ScalarSize::Size8, false) => VectorSize::Size8x8,
            (ScalarSize::Size8, true) => VectorSize::Size8x16,
            (ScalarSize::Size16, false) => VectorSize::Size16x4,
            (ScalarSize::Size16, true) => VectorSize::Size16x8,
            (ScalarSize::Size32, false) => VectorSize::Size32x2,
            (ScalarSize::Size32, true) => VectorSize::Size32x4,
            (ScalarSize::Size64, true) => VectorSize::Size64x2,
            _ => panic!("Unexpected scalar FP operand size: {:?}", size),
        }
    }

    /// Convert from a type into a vector operand size.
    pub fn from_ty(ty: Type) -> VectorSize {
        debug_assert!(ty.is_vector());

        match ty {
            B8X16 => VectorSize::Size8x16,
            B16X8 => VectorSize::Size16x8,
            B32X4 => VectorSize::Size32x4,
            B64X2 => VectorSize::Size64x2,
            F32X2 => VectorSize::Size32x2,
            F32X4 => VectorSize::Size32x4,
            F64X2 => VectorSize::Size64x2,
            I8X8 => VectorSize::Size8x8,
            I8X16 => VectorSize::Size8x16,
            I16X4 => VectorSize::Size16x4,
            I16X8 => VectorSize::Size16x8,
            I32X2 => VectorSize::Size32x2,
            I32X4 => VectorSize::Size32x4,
            I64X2 => VectorSize::Size64x2,
            _ => unimplemented!("Unsupported type: {}", ty),
        }
    }

    /// Get the integer operand size that corresponds to a lane of a vector with a certain size.
    pub fn operand_size(&self) -> OperandSize {
        match self {
            VectorSize::Size64x2 => OperandSize::Size64,
            _ => OperandSize::Size32,
        }
    }

    /// Get the scalar operand size that corresponds to a lane of a vector with a certain size.
    pub fn lane_size(&self) -> ScalarSize {
        match self {
            VectorSize::Size8x8 | VectorSize::Size8x16 => ScalarSize::Size8,
            VectorSize::Size16x4 | VectorSize::Size16x8 => ScalarSize::Size16,
            VectorSize::Size32x2 | VectorSize::Size32x4 => ScalarSize::Size32,
            VectorSize::Size64x2 => ScalarSize::Size64,
        }
    }

    pub fn is_128bits(&self) -> bool {
        match self {
            VectorSize::Size8x8 => false,
            VectorSize::Size8x16 => true,
            VectorSize::Size16x4 => false,
            VectorSize::Size16x8 => true,
            VectorSize::Size32x2 => false,
            VectorSize::Size32x4 => true,
            VectorSize::Size64x2 => true,
        }
    }

    /// Return the encoding bits that are used by some SIMD instructions
    /// for a particular operand size.
    pub fn enc_size(&self) -> (u32, u32) {
        let q = self.is_128bits() as u32;
        let size = match self.lane_size() {
            ScalarSize::Size8 => 0b00,
            ScalarSize::Size16 => 0b01,
            ScalarSize::Size32 => 0b10,
            ScalarSize::Size64 => 0b11,
            _ => unreachable!(),
        };

        (q, size)
    }
}

pub(crate) fn dynamic_to_fixed(ty: Type) -> Type {
    match ty {
        I8X8XN => I8X8,
        I8X16XN => I8X16,
        I16X4XN => I16X4,
        I16X8XN => I16X8,
        I32X2XN => I32X2,
        I32X4XN => I32X4,
        I64X2XN => I64X2,
        F32X4XN => F32X4,
        F64X2XN => F64X2,
        _ => unreachable!("unhandled type: {}", ty),
    }
}
