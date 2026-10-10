use std::mem;

use quote::ToTokens;
use syn::{
    parenthesized,
    parse::{Parse, ParseStream},
    punctuated::{Pair, Punctuated},
    Attribute, Expr, Ident, Token, Visibility,
};

mod display;
mod parsing;

#[derive(Clone)]
pub(super) enum FieldType {
    Concrete(syn::Type),
    Structure(StructSpec),
}
impl FieldType {
    pub(super) fn ty(&self) -> syn::Type {
        match self {
            FieldType::Concrete(t) => t.clone(),
            FieldType::Structure(s) => syn::parse2(s.ident.to_token_stream()).unwrap(),
        }
    }
}

#[derive(Clone)]
pub(super) struct FieldSpec {
    pub(super) attributes: Vec<Attribute>,
    is_validated_map: bool,
    pub(super) vis: Visibility,
    pub(super) ident: Ident,
    pub(super) ty: FieldType,
    pub(super) constraint: Option<Expr>,
}
impl FieldSpec {
    pub(super) fn recursive_accessors(&self) -> bool {
        if let FieldType::Structure(_) = &self.ty {
            true
        } else {
            self.is_validated_map
        }
    }
}
#[derive(Clone)]
pub(super) struct StructSpec {
    #[allow(dead_code)]
    visibility: Visibility,
    pub(super) attrs: Vec<Attribute>,
    recursive_attrs: Vec<Attribute>,
    pub(super) ident: Ident,
    pub(super) fields: Punctuated<FieldSpec, Token![,]>,
}
impl StructSpec {
    fn flatten_go(
        mut self,
        list: &mut Vec<Self>,
        recursive_attrs: &mut Vec<Vec<Attribute>>,
    ) -> Self {
        let mut self_rec_attrs = Vec::new();
        mem::swap(&mut self_rec_attrs, &mut self.recursive_attrs);
        recursive_attrs.push(self_rec_attrs);
        self.attrs.extend(recursive_attrs.iter().flatten().cloned());
        self.fields = mem::take(&mut self.fields)
            .into_pairs()
            .map(|pair| {
                let (mut field, punctuation) = pair.into_tuple();
                field.ty = match field.ty {
                    FieldType::Concrete(ty) => FieldType::Concrete(ty),
                    FieldType::Structure(nested) => {
                        FieldType::Structure(nested.flatten_go(list, recursive_attrs))
                    }
                };
                Pair::new(field, punctuation)
            })
            .collect();
        recursive_attrs.pop();
        list.push(self.clone());
        self
    }
    pub(super) fn flatten(self) -> Vec<Self> {
        let mut list = Vec::with_capacity(1);
        let mut rec_attrs = Vec::with_capacity(1);
        self.flatten_go(&mut list, &mut rec_attrs);
        list
    }
}

#[cfg(test)]
mod flattening_tests {
    use super::{FieldSpec, FieldType, StructSpec};
    use quote::{quote, ToTokens};
    fn assert_structure_eq(actual: &StructSpec, expected: &StructSpec) {
        let actual_visibility = &actual.visibility;
        let expected_visibility = &expected.visibility;
        assert_eq!(
            quote!(#actual_visibility).to_string(),
            quote!(#expected_visibility).to_string()
        );
        assert_eq!(actual.ident, expected.ident);
        let actual_attrs = &actual.attrs;
        let expected_attrs = &expected.attrs;
        assert_eq!(
            quote!(#(#actual_attrs)*).to_string(),
            quote!(#(#expected_attrs)*).to_string()
        );
        let actual_recursive_attrs = &actual.recursive_attrs;
        let expected_recursive_attrs = &expected.recursive_attrs;
        assert_eq!(
            quote!(#(#actual_recursive_attrs)*).to_string(),
            quote!(#(#expected_recursive_attrs)*).to_string()
        );
        assert_eq!(actual.fields.len(), expected.fields.len());
        for (actual_pair, expected_pair) in actual.fields.pairs().zip(expected.fields.pairs()) {
            assert_eq!(
                actual_pair
                    .punct()
                    .map(ToTokens::to_token_stream)
                    .map(|tokens| tokens.to_string()),
                expected_pair
                    .punct()
                    .map(ToTokens::to_token_stream)
                    .map(|tokens| tokens.to_string())
            );
            let actual_field = actual_pair.value();
            let expected_field = expected_pair.value();
            assert_eq!(actual_field.ident, expected_field.ident);
            assert_eq!(
                actual_field.is_validated_map,
                expected_field.is_validated_map
            );
            let actual_visibility = &actual_field.vis;
            let expected_visibility = &expected_field.vis;
            assert_eq!(
                quote!(#actual_visibility).to_string(),
                quote!(#expected_visibility).to_string()
            );
            let actual_attrs = &actual_field.attributes;
            let expected_attrs = &expected_field.attributes;
            assert_eq!(
                quote!(#(#actual_attrs)*).to_string(),
                quote!(#(#expected_attrs)*).to_string()
            );
            let actual_constraint = &actual_field.constraint;
            let expected_constraint = &expected_field.constraint;
            assert_eq!(
                quote!(#actual_constraint).to_string(),
                quote!(#expected_constraint).to_string()
            );
            match (&actual_field.ty, &expected_field.ty) {
                (FieldType::Concrete(actual_type), FieldType::Concrete(expected_type)) => {
                    assert_eq!(
                        quote!(#actual_type).to_string(),
                        quote!(#expected_type).to_string()
                    )
                }
                (FieldType::Structure(actual_child), FieldType::Structure(expected_child)) => {
                    assert_structure_eq(actual_child, expected_child)
                }
                _ => panic!("flattening changed the field type alternative"),
            }
        }
    }
    #[test]
    fn nested_structure_moves_through_actual_flattening() {
        let source: StructSpec = syn::parse2(quote!(Service {
            transport: Transport {}
        }))
        .unwrap();
        let mut structures = Vec::new();
        let mut recursive_attrs = Vec::new();
        let retained = source.flatten_go(&mut structures, &mut recursive_attrs);
        assert_eq!(retained.ident.to_string(), "Service");
        assert_eq!(
            structures
                .iter()
                .map(|structure| structure.ident.to_string())
                .collect::<Vec<_>>(),
            ["Transport", "Service"]
        );
        assert!(recursive_attrs.is_empty());
    }
    #[test]
    fn flattening_preserves_complete_nested_syntax_and_attribute_scope() {
        let source: StructSpec = syn::parse2(quote! {
            pub(crate) #[root_local] #[recursive_attrs] #[root_inherited]
            Service {
                #[port_note] pub port: u16 where (valid_port),
                pub(crate) transport: pub(super) #[transport_local]
                #[recursive_attrs] #[transport_inherited] Transport {
                    tls: #[tls_local] #[recursive_attrs] #[tls_inherited] Tls {
                        #[certificate_note] pub certificate: Vec<u8> where (valid_certificate),
                    },
                    pause: [u64; 2]
                },
                empty: #[empty_local] Empty {},
                #[validated(recursive_accessors)] settings: Settings,
                count: (u8, u16),
            }
        })
        .unwrap();
        let expected: StructSpec = syn::parse2(quote! {
            pub(crate) #[root_local] #[root_inherited]
            Service {
                #[port_note] pub port: u16 where (valid_port),
                pub(crate) transport: pub(super) #[transport_local]
                #[root_inherited] #[transport_inherited] Transport {
                    tls: #[tls_local] #[root_inherited] #[transport_inherited]
                    #[tls_inherited] Tls {
                        #[certificate_note] pub certificate: Vec<u8> where (valid_certificate),
                    },
                    pause: [u64; 2]
                },
                empty: #[empty_local] #[root_inherited] Empty {},
                #[validated(recursive_accessors)] settings: Settings,
                count: (u8, u16),
            }
        })
        .unwrap();
        let FieldType::Structure(expected_transport) = &expected.fields[1].ty else {
            panic!("expected transport structure")
        };
        let FieldType::Structure(expected_tls) = &expected_transport.fields[0].ty else {
            panic!("expected TLS structure")
        };
        let FieldType::Structure(expected_empty) = &expected.fields[2].ty else {
            panic!("expected empty structure")
        };
        let structures = source.flatten();
        let retained = structures.last().unwrap();
        assert_eq!(structures.len(), 4);
        for (actual, expected) in
            structures
                .iter()
                .zip([expected_tls, expected_transport, expected_empty, &expected])
        {
            assert_structure_eq(actual, expected);
        }
        let projected_types = retained
            .fields
            .iter()
            .map(|field| field.ty.ty().to_token_stream().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            projected_types,
            ["u16", "Transport", "Empty", "Settings", "(u8 , u16)"]
        );
        let recursive_accessors = retained
            .fields
            .iter()
            .map(FieldSpec::recursive_accessors)
            .collect::<Vec<_>>();
        assert_eq!(recursive_accessors, [false, true, true, true, false]);
    }
}
