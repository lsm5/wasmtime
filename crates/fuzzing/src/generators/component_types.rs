//! This module generates test cases for the Wasmtime component model function APIs,
//! e.g. `wasmtime::component::func::Func` and `TypedFunc`.
//!
//! Each case includes a list of arbitrary interface types to use as parameters, plus another one to use as a
//! result, and a component which exports a function and imports a function.  The exported function forwards its
//! parameters to the imported one and forwards the result back to the caller.  This serves to excercise Wasmtime's
//! lifting and lowering code and verify the values remain intact during both processes.

use arbitrary::{Arbitrary, Unstructured};
use component_fuzz_util::{Declarations, EXPORT_FUNCTION, IMPORT_FUNCTION};
use std::fmt::Debug;
use std::ops::ControlFlow;
use wasmtime::component::{self, Component, Lift, Linker, Lower, Val};
use wasmtime::{Config, Engine, Store, StoreContextMut};

/// Minimum length of an arbitrary list value generated for a test case
const MIN_LIST_LENGTH: u32 = 0;

/// Maximum length of an arbitrary list value generated for a test case
const MAX_LIST_LENGTH: u32 = 10;

/// Generate an arbitrary instance of the specified type.
pub fn arbitrary_val(ty: &component::Type, input: &mut Unstructured) -> arbitrary::Result<Val> {
    use component::Type;

    Ok(match ty {
        Type::Unit => Val::Unit,
        Type::Bool => Val::Bool(input.arbitrary()?),
        Type::S8 => Val::S8(input.arbitrary()?),
        Type::U8 => Val::U8(input.arbitrary()?),
        Type::S16 => Val::S16(input.arbitrary()?),
        Type::U16 => Val::U16(input.arbitrary()?),
        Type::S32 => Val::S32(input.arbitrary()?),
        Type::U32 => Val::U32(input.arbitrary()?),
        Type::S64 => Val::S64(input.arbitrary()?),
        Type::U64 => Val::U64(input.arbitrary()?),
        Type::Float32 => Val::Float32(input.arbitrary::<f32>()?.to_bits()),
        Type::Float64 => Val::Float64(input.arbitrary::<f64>()?.to_bits()),
        Type::Char => Val::Char(input.arbitrary()?),
        Type::String => Val::String(input.arbitrary()?),
        Type::List(list) => {
            let mut values = Vec::new();
            input.arbitrary_loop(Some(MIN_LIST_LENGTH), Some(MAX_LIST_LENGTH), |input| {
                values.push(arbitrary_val(&list.ty(), input)?);

                Ok(ControlFlow::Continue(()))
            })?;

            list.new_val(values.into()).unwrap()
        }
        Type::Record(record) => record
            .new_val(
                record
                    .fields()
                    .map(|field| Ok((field.name, arbitrary_val(&field.ty, input)?)))
                    .collect::<arbitrary::Result<Vec<_>>>()?,
            )
            .unwrap(),
        Type::Tuple(tuple) => tuple
            .new_val(
                tuple
                    .types()
                    .map(|ty| arbitrary_val(&ty, input))
                    .collect::<arbitrary::Result<_>>()?,
            )
            .unwrap(),
        Type::Variant(variant) => {
            let mut cases = variant.cases();
            let discriminant = input.int_in_range(0..=cases.len() - 1)?;
            variant
                .new_val(
                    &format!("C{discriminant}"),
                    arbitrary_val(&cases.nth(discriminant).unwrap().ty, input)?,
                )
                .unwrap()
        }
        Type::Enum(en) => {
            let discriminant = input.int_in_range(0..=en.names().len() - 1)?;
            en.new_val(&format!("C{discriminant}")).unwrap()
        }
        Type::Union(un) => {
            let mut types = un.types();
            let discriminant = input.int_in_range(0..=types.len() - 1)?;
            un.new_val(
                discriminant.try_into().unwrap(),
                arbitrary_val(&types.nth(discriminant).unwrap(), input)?,
            )
            .unwrap()
        }
        Type::Option(option) => {
            let discriminant = input.int_in_range(0..=1)?;
            option
                .new_val(match discriminant {
                    0 => None,
                    1 => Some(arbitrary_val(&option.ty(), input)?),
                    _ => unreachable!(),
                })
                .unwrap()
        }
        Type::Expected(expected) => {
            let discriminant = input.int_in_range(0..=1)?;
            expected
                .new_val(match discriminant {
                    0 => Ok(arbitrary_val(&expected.ok(), input)?),
                    1 => Err(arbitrary_val(&expected.err(), input)?),
                    _ => unreachable!(),
                })
                .unwrap()
        }
        Type::Flags(flags) => flags
            .new_val(
                &flags
                    .names()
                    .filter_map(|name| {
                        input
                            .arbitrary()
                            .map(|p| if p { Some(name) } else { None })
                            .transpose()
                    })
                    .collect::<arbitrary::Result<Box<[_]>>>()?,
            )
            .unwrap(),
    })
}

macro_rules! define_static_api_test {
    ($name:ident $(($param:ident $param_name:ident $param_expected_name:ident))*) => {
        #[allow(unused_parens)]
        /// Generate zero or more sets of arbitrary argument and result values and execute the test using those
        /// values, asserting that they flow from host-to-guest and guest-to-host unchanged.
        pub fn $name<'a, $($param,)* R>(
            input: &mut Unstructured<'a>,
            declarations: &Declarations,
        ) -> arbitrary::Result<()>
        where
            $($param: Lift + Lower + Clone + PartialEq + Debug + Arbitrary<'a> + 'static,)*
            R: Lift + Lower + Clone + PartialEq + Debug + Arbitrary<'a> + 'static
        {
            crate::init_fuzzing();

            let mut config = Config::new();
            config.wasm_component_model(true);
            let engine = Engine::new(&config).unwrap();
            let component = Component::new(
                &engine,
                declarations.make_component().as_bytes()
            ).unwrap();
            let mut linker = Linker::new(&engine);
            linker
                .root()
                .func_wrap(
                    IMPORT_FUNCTION,
                    |cx: StoreContextMut<'_, ($(Option<$param>,)* Option<R>)>,
                    $($param_name: $param,)*|
                    {
                        let ($($param_expected_name,)* result) = cx.data();
                        $(assert_eq!($param_name, *$param_expected_name.as_ref().unwrap());)*
                        Ok(result.as_ref().unwrap().clone())
                    },
                )
                .unwrap();
            let mut store = Store::new(&engine, Default::default());
            let instance = linker.instantiate(&mut store, &component).unwrap();
            let func = instance
                .get_typed_func::<($($param,)*), R, _>(&mut store, EXPORT_FUNCTION)
                .unwrap();

            while input.arbitrary()? {
                $(let $param_name = input.arbitrary::<$param>()?;)*
                let result = input.arbitrary::<R>()?;
                *store.data_mut() = ($(Some($param_name.clone()),)* Some(result.clone()));

                assert_eq!(func.call(&mut store, ($($param_name,)*)).unwrap(), result);
                func.post_return(&mut store).unwrap();
            }

            Ok(())
        }
    }
}

define_static_api_test!(static_api_test0);
define_static_api_test!(static_api_test1 (P0 p0 p0_expected));
define_static_api_test!(static_api_test2 (P0 p0 p0_expected) (P1 p1 p1_expected));
define_static_api_test!(static_api_test3 (P0 p0 p0_expected) (P1 p1 p1_expected) (P2 p2 p2_expected));
define_static_api_test!(static_api_test4 (P0 p0 p0_expected) (P1 p1 p1_expected) (P2 p2 p2_expected)
                        (P3 p3 p3_expected));
define_static_api_test!(static_api_test5 (P0 p0 p0_expected) (P1 p1 p1_expected) (P2 p2 p2_expected)
                        (P3 p3 p3_expected) (P4 p4 p4_expected));
