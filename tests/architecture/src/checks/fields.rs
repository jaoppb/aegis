//! Struct rules: at most four fields, no `pub` fields, and in `domain` and `application` no
//! primitive field and no collection mixed with other fields in a multi-field struct.

use syn::{ItemStruct, PathSegment, Type, Visibility};

use crate::violation::{Location, Rule, Violation};
use crate::workspace::{Layer, SourceFile, Workspace};

/// The maximum number of fields in a struct.
pub const MAX_FIELDS: usize = 4;

const PRIMITIVES: [&str; 21] = [
    "String", "str", "bool", "char", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16",
    "u32", "u64", "u128", "usize", "f32", "f64", "PathBuf", "Path", "OsString",
];
const WRAPPERS: [&str; 3] = ["Option", "Vec", "Box"];
const COLLECTIONS: [&str; 6] = [
    "Vec", "VecDeque", "HashMap", "BTreeMap", "HashSet", "BTreeSet",
];
const SERDE_DERIVES: [&str; 2] = ["Serialize", "Deserialize"];

/// Applies the field count, public field, primitive field and collection field rules to crate
/// `src` code.
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    let production = workspace
        .sources()
        .iter()
        .filter(|source| source.is_production());
    production.flat_map(violations_in).collect()
}

fn violations_in(source: &SourceFile) -> Vec<Violation> {
    let structs = source.structs();
    structs
        .into_iter()
        .flat_map(|item| struct_violations(source, item))
        .collect()
}

fn struct_violations(source: &SourceFile, item: &ItemStruct) -> Vec<Violation> {
    let location = Location::new(source.path(), item.ident.span().start().line);
    let modelled = source.layer().is_modelled() && item.fields.len() > 1;
    let findings = [
        count_finding(item, &location),
        primitive_finding(item, &location).filter(|_| modelled),
        collection_finding(item, &location).filter(|_| modelled),
        public_finding(source.layer(), item, &location),
    ];
    findings.into_iter().flatten().collect()
}

fn count_finding(item: &ItemStruct, location: &Location) -> Option<Violation> {
    let count = item.fields.len();
    let message = format!(
        "struct {} has {count} fields (max {MAX_FIELDS})",
        item.ident
    );
    let violation = Violation::new(Rule::StructFields, location.clone(), message);
    (count > MAX_FIELDS).then_some(violation)
}

fn primitive_finding(item: &ItemStruct, location: &Location) -> Option<Violation> {
    let field = item.fields.iter().find(|field| is_primitive(&field.ty))?;
    let name = field
        .ident
        .as_ref()
        .map_or_else(String::new, ToString::to_string);
    let message = format!(
        "field {name} of {} is a primitive; wrap it in a value object",
        item.ident
    );
    Some(Violation::new(
        Rule::PrimitiveField,
        location.clone(),
        message,
    ))
}

fn collection_finding(item: &ItemStruct, location: &Location) -> Option<Violation> {
    let mixes = item.fields.iter().any(|field| is_collection(&field.ty));
    let message = format!(
        "struct {} mixes a collection with other fields; make the collection its own type",
        item.ident
    );
    let violation = Violation::new(Rule::CollectionField, location.clone(), message);
    mixes.then_some(violation)
}

/// Fields are private. The one exception is a wire struct of the adapters, which only mirrors a
/// file format through serde.
fn public_finding(layer: Layer, item: &ItemStruct, location: &Location) -> Option<Violation> {
    if layer == Layer::Adapters && derives_serde(item) {
        return None;
    }
    let exposed = item.fields.iter().any(|field| is_exposed(&field.vis));
    let message = format!(
        "struct {} has a public field; keep fields private",
        item.ident
    );
    let violation = Violation::new(Rule::PubField, location.clone(), message);
    exposed.then_some(violation)
}

const fn is_exposed(visibility: &Visibility) -> bool {
    !matches!(visibility, Visibility::Inherited)
}

fn derives_serde(item: &ItemStruct) -> bool {
    let derives = item
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("derive"));
    derives
        .filter_map(|attribute| derived_names(attribute).ok())
        .flatten()
        .any(|name| SERDE_DERIVES.contains(&name.as_str()))
}

fn derived_names(attribute: &syn::Attribute) -> syn::Result<Vec<String>> {
    let mut names = Vec::new();
    attribute.parse_nested_meta(|meta| {
        let last = meta
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        names.extend(last);
        Ok(())
    })?;
    Ok(names)
}

fn last_segment(field_type: &Type) -> Option<&PathSegment> {
    match field_type {
        Type::Path(path) => path.path.segments.last(),
        _ => None,
    }
}

fn is_primitive(field_type: &Type) -> bool {
    if let Type::Reference(reference) = field_type {
        return is_primitive(&reference.elem);
    }
    let Some(segment) = last_segment(field_type) else {
        return false;
    };
    let name = segment.ident.to_string();
    PRIMITIVES.contains(&name.as_str())
        || WRAPPERS.contains(&name.as_str()) && generic_types(segment).any(is_primitive)
}

fn generic_types(segment: &PathSegment) -> impl Iterator<Item = &Type> {
    let arguments = match &segment.arguments {
        syn::PathArguments::AngleBracketed(arguments) => Some(arguments),
        _ => None,
    };
    arguments
        .into_iter()
        .flat_map(|arguments| arguments.args.iter())
        .filter_map(as_type)
}

const fn as_type(argument: &syn::GenericArgument) -> Option<&Type> {
    match argument {
        syn::GenericArgument::Type(field_type) => Some(field_type),
        _ => None,
    }
}

fn is_collection(field_type: &Type) -> bool {
    match field_type {
        Type::Slice(_) | Type::Array(_) => true,
        Type::Reference(reference) => is_collection(&reference.elem),
        _ => last_segment(field_type)
            .is_some_and(|segment| COLLECTIONS.contains(&segment.ident.to_string().as_str())),
    }
}
