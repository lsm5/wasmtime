use crate::component::{
    Component, ComponentTypes, LowerImport, LoweredIndex, RuntimeAlwaysTrapIndex,
};
use crate::{PrimaryMap, SignatureIndex, Trampoline, WasmFuncType};
use anyhow::Result;
use object::write::Object;
use serde::{Deserialize, Serialize};
use std::any::Any;

/// Description of where a trampoline is located in the text section of a
/// compiled image.
#[derive(Serialize, Deserialize)]
pub struct FunctionInfo {
    /// The byte offset from the start of the text section where this trampoline
    /// starts.
    pub start: u32,
    /// The byte length of this trampoline's function body.
    pub length: u32,
}

/// Description of an "always trap" function generated by
/// `ComponentCompiler::compile_always_trap`.
#[derive(Serialize, Deserialize)]
pub struct AlwaysTrapInfo {
    /// Information about the extent of this generated function.
    pub info: FunctionInfo,
    /// The offset from `start` of where the trapping instruction is located.
    pub trap_offset: u32,
}

/// Compilation support necessary for components.
pub trait ComponentCompiler: Send + Sync {
    /// Creates a trampoline for a `canon.lower`'d host function.
    ///
    /// This function will create a suitable trampoline which can be called from
    /// WebAssembly code and which will then call into host code. The signature
    /// of this generated trampoline should have the appropriate wasm ABI for
    /// the `lowering.canonical_abi` type signature (e.g. System-V).
    ///
    /// The generated trampoline will interpret its first argument as a
    /// `*mut VMComponentContext` and use the `VMComponentOffsets` for
    /// `component` to read necessary data (as specified by `lowering.options`)
    /// and call the host function pointer. Notably the host function pointer
    /// has the signature `VMLoweringCallee` where many of the arguments are
    /// loaded from known offsets (for this particular generated trampoline)
    /// from the `VMComponentContext`.
    ///
    /// Returns a compiler-specific `Box<dyn Any>` which can be passed later to
    /// `emit_obj` to crate an elf object.
    fn compile_lowered_trampoline(
        &self,
        component: &Component,
        lowering: &LowerImport,
        types: &ComponentTypes,
    ) -> Result<Box<dyn Any + Send>>;

    /// Creates a function which will always trap that has the `ty` specified.
    ///
    /// This will create a small trampoline whose only purpose is to generate a
    /// trap at runtime. This is used to implement the degenerate case of a
    /// `canon lift`'d function immediately being `canon lower`'d.
    fn compile_always_trap(&self, ty: &WasmFuncType) -> Result<Box<dyn Any + Send>>;

    /// Emits the `lowerings` and `trampolines` specified into the in-progress
    /// ELF object specified by `obj`.
    ///
    /// Returns a map of trampoline information for where to find them all in
    /// the text section.
    ///
    /// Note that this will also prepare unwinding information for all the
    /// trampolines as necessary.
    fn emit_obj(
        &self,
        lowerings: PrimaryMap<LoweredIndex, Box<dyn Any + Send>>,
        always_trap: PrimaryMap<RuntimeAlwaysTrapIndex, Box<dyn Any + Send>>,
        tramplines: Vec<(SignatureIndex, Box<dyn Any + Send>)>,
        obj: &mut Object<'static>,
    ) -> Result<(
        PrimaryMap<LoweredIndex, FunctionInfo>,
        PrimaryMap<RuntimeAlwaysTrapIndex, AlwaysTrapInfo>,
        Vec<Trampoline>,
    )>;
}
