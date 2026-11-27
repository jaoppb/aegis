//! Finds tuple types and tuple patterns, looking through every wrapper.
//!
//! A tuple is a type or pattern with at least one element. The unit type `()`,
//! a parenthesised single type such as `&(dyn Store + Send)` and the argument
//! list of `Fn(A, B) -> C` are not tuples, but a tuple inside any of them is.

use syn::{
    GenericArgument, Pat, PathArguments, PathSegment, ReturnType, Type, TypeBareFn, TypeParamBound,
    TypePath,
};

/// Whether `candidate` is, or contains anywhere inside it, a tuple type.
#[must_use]
pub fn holds_tuple(candidate: &Type) -> bool {
    match candidate {
        Type::Tuple(tuple) => !tuple.elems.is_empty(),
        Type::Array(inner) => holds_tuple(&inner.elem),
        Type::Slice(inner) => holds_tuple(&inner.elem),
        Type::Paren(inner) => holds_tuple(&inner.elem),
        Type::Group(inner) => holds_tuple(&inner.elem),
        Type::Ptr(inner) => holds_tuple(&inner.elem),
        Type::Reference(inner) => holds_tuple(&inner.elem),
        Type::Path(path) => path_type_holds_tuple(path),
        Type::ImplTrait(inner) => bounds_hold_tuple(inner.bounds.iter()),
        Type::TraitObject(inner) => bounds_hold_tuple(inner.bounds.iter()),
        Type::BareFn(inner) => bare_fn_holds_tuple(inner),
        _ => false,
    }
}

/// Whether the return type `output` holds a tuple.
#[must_use]
pub fn output_holds_tuple(output: &ReturnType) -> bool {
    match output {
        ReturnType::Type(_, returned) => holds_tuple(returned),
        ReturnType::Default => false,
    }
}

/// Whether `candidate` is, or wraps, a tuple pattern.
#[must_use]
pub fn pattern_holds_tuple(candidate: &Pat) -> bool {
    match candidate {
        Pat::Tuple(tuple) => !tuple.elems.is_empty(),
        Pat::Paren(inner) => pattern_holds_tuple(&inner.pat),
        Pat::Reference(inner) => pattern_holds_tuple(&inner.pat),
        Pat::Type(inner) => pattern_holds_tuple(&inner.pat),
        _ => false,
    }
}

fn bare_fn_holds_tuple(pointer: &TypeBareFn) -> bool {
    let inputs = pointer.inputs.iter().any(|input| holds_tuple(&input.ty));
    inputs || output_holds_tuple(&pointer.output)
}

fn path_type_holds_tuple(path: &TypePath) -> bool {
    let qualified = path.qself.iter().any(|qself| holds_tuple(&qself.ty));
    qualified || path.path.segments.iter().any(segment_holds_tuple)
}

fn segment_holds_tuple(segment: &PathSegment) -> bool {
    match &segment.arguments {
        PathArguments::None => false,
        PathArguments::AngleBracketed(angled) => angled.args.iter().any(argument_holds_tuple),
        PathArguments::Parenthesized(sugar) => {
            sugar.inputs.iter().any(holds_tuple) || output_holds_tuple(&sugar.output)
        }
    }
}

fn argument_holds_tuple(argument: &GenericArgument) -> bool {
    match argument {
        GenericArgument::Type(inner) => holds_tuple(inner),
        GenericArgument::AssocType(binding) => holds_tuple(&binding.ty),
        GenericArgument::Constraint(constraint) => bounds_hold_tuple(constraint.bounds.iter()),
        _ => false,
    }
}

fn bounds_hold_tuple<'ast>(mut bounds: impl Iterator<Item = &'ast TypeParamBound>) -> bool {
    bounds.any(bound_holds_tuple)
}

fn bound_holds_tuple(bound: &TypeParamBound) -> bool {
    match bound {
        TypeParamBound::Trait(each) => each.path.segments.iter().any(segment_holds_tuple),
        _ => false,
    }
}
