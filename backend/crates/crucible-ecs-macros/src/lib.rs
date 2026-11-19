//! Procedural macros for `crucible-ecs`.
//!
//! Use the `crucible_ecs::networked` re-export. Generated code uses paths under `::crucible_ecs`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Maximum number of fields with per-field deltas. The mask uses `u64`, like Robust's `ChangedFields`.
const MAX_FIELDS: usize = 64;

/// Makes a component replicable with per-field deltas, equivalent to Robust's `[AutoNetworkedField]`.
///
/// Generates the following for a struct with named fields:
/// - An implementation of `crucible_ecs::NetworkedComponent` with the replicated field names.
/// - For each field `x`, a mask constant `FIELD_X: FieldMask` with a bit at the field's index.
/// - For each field `x`, a setter `fn set_x(world: &mut World, uid: EntityUid, value: T) -> bool`
///   that writes the value and marks that field as changed in the current tick.
///
/// A field annotated with `#[networked(skip)]` has no replicated bit, constant, or setter.
/// At most 64 fields support deltas. Implement `Component` separately.
#[proc_macro_attribute]
pub fn networked(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = TokenStream2::from(attr);
    let input = parse_macro_input!(item as DeriveInput);
    match expand(attr, input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Parses and expands tokens separately from `proc_macro` for unit testing.
fn expand(attr: TokenStream2, mut input: DeriveInput) -> syn::Result<TokenStream2> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "#[networked] on a struct takes no arguments",
        ));
    }
    let Data::Struct(data) = &mut input.data else {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[networked] requires a struct with named fields",
        ));
    };
    let Fields::Named(fields) = &mut data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[networked] requires a struct with named fields",
        ));
    };

    let mut networked = Vec::new();
    for field in fields.named.iter_mut() {
        let skip = take_skip_attr(&mut field.attrs)?;
        if !skip {
            networked.push((
                field.ident.clone().expect("named field"),
                field.ty.clone(),
                field.vis.clone(),
            ));
        }
    }
    if networked.len() > MAX_FIELDS {
        return Err(syn::Error::new_spanned(
            &input.ident,
            format!(
                "#[networked]: {} replicated fields exceed the {MAX_FIELDS}-field limit for a u64 mask",
                networked.len()
            ),
        ));
    }

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let mut names = Vec::new();
    let mut items = Vec::new();
    for (bit, (ident, ty, vis)) in networked.iter().enumerate() {
        let plain = ident.unraw().to_string();
        let const_ident = format_ident!("FIELD_{}", plain.to_uppercase());
        let setter = format_ident!("set_{}", plain);
        let bit = u8::try_from(bit).expect("at most 64 fields");
        let doc_const = format!("Change-mask bit for the field `{plain}`.");
        let doc_setter = format!(
            "Sets `{plain}` and marks it as changed in the current tick. \
              Returns `false` when the entity lacks this component."
        );
        names.push(plain);
        items.push(quote! {
            #[doc = #doc_const]
            #[allow(dead_code)]
            #vis const #const_ident: ::crucible_ecs::FieldMask = ::crucible_ecs::FieldMask::bit(#bit);

            #[doc = #doc_setter]
            #[allow(dead_code)]
            #vis fn #setter(
                world: &mut ::crucible_ecs::World,
                uid: ::crucible_ecs::EntityUid,
                value: #ty,
            ) -> bool {
                match world.get_mut::<Self>(uid) {
                    ::core::option::Option::Some(component) => component.#ident = value,
                    ::core::option::Option::None => return false,
                }
                world.dirty_fields::<Self>(uid, Self::#const_ident)
            }
        });
    }

    Ok(quote! {
        #input

        impl #impl_generics #name #ty_generics #where_clause {
            #(#items)*
        }

        impl #impl_generics ::crucible_ecs::NetworkedComponent for #name #ty_generics #where_clause {
            const FIELD_NAMES: &'static [&'static str] = &[#(#names),*];
        }
    })
}

/// Removes the field's `#[networked(...)]` attributes and reports whether one specified `skip`.
fn take_skip_attr(attrs: &mut Vec<syn::Attribute>) -> syn::Result<bool> {
    let mut skip = false;
    let mut error = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("networked") {
            return true;
        }
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                skip = true;
                Ok(())
            } else {
                Err(meta.error("a field accepts only #[networked(skip)]"))
            }
        });
        if let Err(e) = parsed {
            error.get_or_insert(e);
        }
        false
    });
    match error {
        Some(e) => Err(e),
        None => Ok(skip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_str(attr: TokenStream2, item: TokenStream2) -> syn::Result<String> {
        let input: DeriveInput = syn::parse2(item)?;
        expand(attr, input).map(|t| t.to_string())
    }

    #[test]
    fn generates_masks_setters_and_field_names() {
        let out = expand_str(
            quote!(),
            quote! {
                pub struct Door { pub open: bool, pub speed: f32, #[networked(skip)] cache: u32 }
            },
        )
        .expect("macro expansion");
        assert!(out.contains("FIELD_OPEN"));
        assert!(out.contains("FieldMask :: bit (0u8)"));
        assert!(out.contains("FIELD_SPEED"));
        assert!(out.contains("FieldMask :: bit (1u8)"));
        assert!(out.contains("fn set_speed"));
        assert!(!out.contains("FIELD_CACHE"));
        assert!(!out.contains("set_cache"));
        assert!(out.contains("& [\"open\" , \"speed\"]"));
        // Strip the helper attribute so the compiler never sees it.
        assert!(!out.contains("networked (skip)"));
    }

    #[test]
    fn raw_identifiers_are_unrawed() {
        let out =
            expand_str(quote!(), quote! { struct S { r#type: u8 } }).expect("macro expansion");
        assert!(out.contains("FIELD_TYPE"));
        assert!(out.contains("set_type"));
    }

    #[test]
    fn rejects_tuple_struct_enum_and_arguments() {
        assert!(expand_str(quote!(), quote! { struct S(u8); }).is_err());
        assert!(expand_str(quote!(), quote! { enum S { A } }).is_err());
        assert!(expand_str(quote!(foo), quote! { struct S { a: u8 } }).is_err());
        assert!(expand_str(quote!(), quote! { struct S { #[networked(other)] a: u8 } }).is_err());
    }

    #[test]
    fn rejects_more_than_64_fields() {
        let fields = (0..65).map(|i| {
            let f = format_ident!("f{i}");
            quote!(#f: u8)
        });
        let err = expand_str(quote!(), quote! { struct S { #(#fields),* } }).unwrap_err();
        assert!(err.to_string().contains("65"));
        let fields = (0..64).map(|i| {
            let f = format_ident!("f{i}");
            quote!(#f: u8)
        });
        assert!(expand_str(quote!(), quote! { struct S { #(#fields),* } }).is_ok());
    }
}
