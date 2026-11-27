//! Tuples are not allowed as ad-hoc groupings.
//!
//! A tuple type or pattern may not appear in a function signature, a type alias or a struct field.
//! Closures, the unit type, parenthesised single types, `Fn(A, B) -> C` argument lists, enum
//! variant payloads and tuple expressions inside bodies are not groupings. Methods of a trait impl
//! are not checked either: the trait definition is the signature that is checked.

pub mod detect;

use std::path::Path;

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Field, FnArg, ImplItem, ItemImpl, ItemMod, ItemStruct, ItemType, Signature};

use crate::checks::tuples::detect::{holds_tuple, output_holds_tuple, pattern_holds_tuple};
use crate::violation::{Location, Rule, Violation};
use crate::workspace::{SourceFile, Workspace, is_test_module};

/// Applies the tuple signature, tuple alias and tuple field rules to crate `src` code.
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    let production = workspace
        .sources()
        .iter()
        .filter(|source| source.is_production());
    production.flat_map(violations_in).collect()
}

fn violations_in(source: &SourceFile) -> Vec<Violation> {
    let mut visitor = TupleVisitor {
        path: source.path(),
        found: Vec::new(),
    };
    visitor.visit_file(source.syntax());
    visitor.found
}

struct TupleVisitor<'path> {
    path: &'path Path,
    found: Vec<Violation>,
}

impl<'ast> Visit<'ast> for TupleVisitor<'_> {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        (!is_test_module(item)).then(|| syn::visit::visit_item_mod(self, item));
    }

    fn visit_signature(&mut self, signature: &'ast Signature) {
        let finding = self.signature_finding(signature);
        self.found.extend(finding);
        syn::visit::visit_signature(self, signature);
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        match item.trait_ {
            None => syn::visit::visit_item_impl(self, item),
            Some(_) => item
                .items
                .iter()
                .for_each(|member| self.visit_member(member)),
        }
    }

    fn visit_item_type(&mut self, item: &'ast ItemType) {
        let finding = self.alias_finding(item);
        self.found.extend(finding);
        syn::visit::visit_item_type(self, item);
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        let findings = self.field_findings(item);
        self.found.extend(findings);
        syn::visit::visit_item_struct(self, item);
    }
}

impl<'ast> TupleVisitor<'_> {
    fn visit_member(&mut self, member: &'ast ImplItem) {
        match member {
            ImplItem::Fn(method) => self.visit_block(&method.block),
            other => self.visit_impl_item(other),
        }
    }

    fn at(&self, span: Span) -> Location {
        Location::new(self.path, span.start().line)
    }

    fn signature_finding(&self, signature: &Signature) -> Option<Violation> {
        let message = format!(
            "function {} has a tuple in its signature; name the grouping with a type",
            signature.ident
        );
        let location = self.at(signature.ident.span());
        signature_holds_tuple(signature)
            .then(|| Violation::new(Rule::TupleSignature, location, message))
    }

    fn alias_finding(&self, item: &ItemType) -> Option<Violation> {
        let message = format!(
            "type alias {} is a tuple; name the grouping with a struct",
            item.ident
        );
        let location = self.at(item.ident.span());
        holds_tuple(&item.ty).then(|| Violation::new(Rule::TupleAlias, location, message))
    }

    fn field_findings(&self, item: &ItemStruct) -> Vec<Violation> {
        let tupled = item.fields.iter().filter(|field| holds_tuple(&field.ty));
        tupled
            .map(|field| self.field_violation(item, field))
            .collect()
    }

    fn field_violation(&self, item: &ItemStruct, field: &Field) -> Violation {
        let message = format!(
            "struct {} has a tuple-typed field; name the grouping with a type",
            item.ident
        );
        Violation::new(Rule::TupleField, self.at(field.ty.span()), message)
    }
}

fn signature_holds_tuple(signature: &Signature) -> bool {
    signature.inputs.iter().any(argument_holds_tuple) || output_holds_tuple(&signature.output)
}

fn argument_holds_tuple(argument: &FnArg) -> bool {
    match argument {
        FnArg::Receiver(receiver) => holds_tuple(&receiver.ty),
        FnArg::Typed(typed) => holds_tuple(&typed.ty) || pattern_holds_tuple(&typed.pat),
    }
}
