use crate::{Derives, TypegenError};

use self::{
    ir::module_ir::ModuleIR,
    ir::type_ir::{CompositeFieldIR, CompositeIR, CompositeIRKind, EnumIR, TypeIR, TypeIRKind},
    settings::TypeGeneratorSettings,
    type_params::TypeParameters,
    type_path::TypeParameter,
    type_path_resolver::TypePathResolver,
};

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use scale_info::{form::PortableForm, PortableRegistry, PortableType, Type, TypeDef};
use syn::parse_quote;

pub mod error;
pub mod ir;
pub mod settings;
pub mod type_params;
pub mod type_path;
pub mod type_path_resolver;

/// An interface for generating a types module.
pub struct TypeGenerator<'a> {
    type_registry: &'a PortableRegistry,
    pub settings: TypeGeneratorSettings,
    root_mod_ident: Ident,
}

impl<'a> TypeGenerator<'a> {
    /// Construct a new [`TypeGenerator`].
    pub fn new(
        type_registry: &'a PortableRegistry,
        settings: TypeGeneratorSettings,
    ) -> Result<Self, TypegenError> {
        let root_mod_ident: Ident = syn::parse_str(&settings.type_mod_name)?;
        Ok(Self {
            type_registry,
            settings,
            root_mod_ident,
        })
    }

    pub fn types_mod_ident(&self) -> &Ident {
        &self.root_mod_ident
    }

    /// Generate a module containing all types defined in the supplied type registry.
    pub fn generate_types_mod(&self) -> Result<ModuleIR, TypegenError> {
        let mut root_mod = ModuleIR::new(self.root_mod_ident.clone(), self.root_mod_ident.clone());

        for ty in &self.type_registry.types {
            let path = &ty.ty.path;
            // Don't generate a type if it was substituted - the target type might
            // not be in the type registry + our resolution already performs the substitution.
            if self.settings.substitutes.contains(path) {
                continue;
            }

            let namespace = path.namespace();
            // prelude types e.g. Option/Result have no namespace, so we don't generate them
            if namespace.is_empty() {
                continue;
            }

            // if the type is not a builtin type, insert it into the respective module
            if let Some(type_ir) = self.create_type_ir(ty)? {
                // Create the module this type should go into
                let innermost_module = root_mod.get_or_insert_submodule(namespace);
                innermost_module.types.insert(path.clone(), type_ir);
            }
        }

        Ok(root_mod)
    }

    fn create_type_ir(&self, ty: &PortableType) -> Result<Option<TypeIR>, TypegenError> {
        let PortableType { ty, id: _ } = &ty;

        // if the type is some builtin, early return, we are only interested in generating structs and enums.
        if !matches!(ty.type_def, TypeDef::Composite(_) | TypeDef::Variant(_)) {
            return Ok(None);
        }

        let mut type_params = TypeParameters::from_scale_info(&ty.type_params);

        let name = ty
            .path
            .ident()
            .map(|e| syn::parse_str::<Ident>(&e))
            .expect(
            "Structs and enums should have a name. Checked with namespace.is_empty() above. qed;",
        )?;

        let docs = self.docs_from_scale_info(&ty.docs);

        let mut could_derive_as_compact: bool = false;
        let kind = match &ty.type_def {
            TypeDef::Composite(composite) => {
                let kind = self.create_composite_ir_kind(&composite.fields, &mut type_params)?;

                if kind.could_derive_as_compact() {
                    could_derive_as_compact = true;
                }

                TypeIRKind::Struct(CompositeIR { name, kind, docs })
            }
            TypeDef::Variant(variant) => {
                let variants = variant
                    .variants
                    .iter()
                    .map(|v| {
                        let name = syn::parse_str::<Ident>(&v.name)?;
                        let kind = self.create_composite_ir_kind(&v.fields, &mut type_params)?;
                        let docs = self.docs_from_scale_info(&v.docs);
                        Ok((v.index, CompositeIR { kind, name, docs }))
                    })
                    .collect::<Result<Vec<(u8, CompositeIR)>, TypegenError>>()?;
                TypeIRKind::Enum(EnumIR {
                    name,
                    variants,
                    docs,
                })
            }
            _ => unreachable!("Other variants early return before. qed."),
        };

        let mut derives = self.type_derives(ty)?;
        if could_derive_as_compact {
            self.add_as_compact_derive(&mut derives);
        }

        let type_ir = TypeIR {
            kind,
            derives,
            type_params,
            insert_codec_attributes: self.settings.insert_codec_attributes,
        };
        Ok(Some(type_ir))
    }

    /// takes into account the settings value for `should_gen_docs`
    pub fn docs_from_scale_info(&self, docs: &[String]) -> TokenStream {
        self.settings
            .should_gen_docs
            .then_some(quote! { #( #[doc = #docs ] )* })
            .unwrap_or_default()
    }

    pub fn create_composite_ir_kind(
        &self,
        fields: &[scale_info::Field<PortableForm>],
        type_params: &mut TypeParameters,
    ) -> Result<CompositeIRKind, TypegenError> {
        let type_path_resolver = self.type_path_resolver();

        if fields.is_empty() {
            return Ok(CompositeIRKind::NoFields);
        }

        let all_fields_named = fields.iter().all(|f| f.name.is_some());
        let all_fields_unnamed = fields.iter().all(|f| f.name.is_none());

        if !(all_fields_named || all_fields_unnamed) {
            return Err(TypegenError::InvalidFields(format!("{:?}", fields)));
        }

        if all_fields_named {
            let named_fields = fields
                .iter()
                .map(|field| {
                    let field_name = field.name.as_ref().unwrap();
                    let ident = syn::parse_str::<Ident>(field_name)?;

                    let path = type_path_resolver.resolve_field_type_path(
                        field.ty.id,
                        type_params.params(),
                        field.type_name.as_deref(),
                    )?;
                    let is_compact = path.is_compact();
                    let is_boxed = field
                        .type_name
                        .as_ref()
                        .map(|e| e.contains("Box<"))
                        .unwrap_or_default();

                    for param in path.parent_type_params().iter() {
                        type_params.mark_used(param);
                    }

                    Ok((ident, CompositeFieldIR::new(path, is_compact, is_boxed)))
                })
                .collect::<Result<Vec<(Ident, CompositeFieldIR)>, TypegenError>>()?;
            Ok(CompositeIRKind::Named(named_fields))
        } else if all_fields_unnamed {
            let unnamed_fields = fields
                .iter()
                .map(|field| {
                    let path = type_path_resolver.resolve_field_type_path(
                        field.ty.id,
                        type_params.params(),
                        field.type_name.as_deref(),
                    )?;

                    let is_compact = path.is_compact();
                    let is_boxed = field
                        .type_name
                        .as_ref()
                        .map(|e| e.contains("Box<"))
                        .unwrap_or_default();

                    for param in path.parent_type_params().iter() {
                        type_params.mark_used(param);
                    }

                    Ok(CompositeFieldIR::new(path, is_compact, is_boxed))
                })
                .collect::<Result<Vec<CompositeFieldIR>, TypegenError>>()?;
            Ok(CompositeIRKind::Unnamed(unnamed_fields))
        } else {
            unreachable!("Is either all unnamed or all named. qed.")
        }
    }

    pub fn type_path_resolver(&self) -> TypePathResolver<'_> {
        TypePathResolver::new(
            self.type_registry,
            &self.settings.substitutes,
            self.settings.decoded_bits_type_path.as_ref(),
            self.settings.compact_type_path.as_ref(),
            &self.root_mod_ident,
        )
    }

    pub fn upcast_composite(&self, composite: &CompositeIR) -> TypeIR {
        // just use Default Derives + AsCompact. No access to type specific derives here. (Mainly used in subxt to create structs from enum variants...)
        let mut derives = self.settings.derives.default_derives().clone();
        if composite.kind.could_derive_as_compact() {
            self.add_as_compact_derive(&mut derives)
        }
        TypeIR {
            type_params: TypeParameters::from_scale_info(&[]),
            derives,
            insert_codec_attributes: self.settings.insert_codec_attributes,
            kind: TypeIRKind::Struct(composite.clone()),
        }
    }

    pub fn default_derives(&self) -> &Derives {
        self.settings.derives.default_derives()
    }

    pub fn type_derives(&self, ty: &Type<PortableForm>) -> Result<Derives, TypegenError> {
        let joined_path = ty.path.segments.join("::");
        let ty_path: syn::TypePath = syn::parse_str(&joined_path)?;
        let derives = self.settings.derives.resolve(&ty_path);
        Ok(derives)
    }

    /// Adds a AsCompact derive, if a path to AsCompact trait/derive macro set in settings.
    fn add_as_compact_derive(&self, derives: &mut Derives) {
        if let Some(compact_as_type_path) = &self.settings.compact_as_type_path {
            derives.insert_derive(parse_quote!(#compact_as_type_path));
        }
    }
}