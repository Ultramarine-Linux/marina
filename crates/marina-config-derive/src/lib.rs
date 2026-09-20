//! `#[derive(ConfigTemplate)]`: renders a commented TOML template of a
//! config struct's defaults, using `///` doc comments as documentation.
//!
//! Like clap's derive, the same annotations also wire up the runtime side:
//! every `#[template(env = "VAR")]` field contributes a
//! `(dotted.path, VAR, is_bool)` binding via `__marina_env_bindings`, so a
//! single hand-written applier can merge environment over the file layer
//! without a repetitive per-field `if let` chain.
//!
//! Supported field shapes:
//!
//! - leaves (anything `Debug`: `bool`, `String`, `PathBuf`, numbers,
//!   `Vec<..>`) render as `key = <Debug of the default>`;
//! - `Option<leaf>` fields render the contained value when `Some`, else a
//!   `# key = <example>` placeholder (`#[template(example = ...)]` is
//!   required so the template stays complete);
//! - struct fields recurse transparently, extending the table path by field
//!   name, unless marked `#[template(table)]`, which emits a `[path.name]`
//!   header first;
//! - `HashMap<String, V>` fields marked `#[template(example = "slug")]`
//!   emit one example `[path."slug"]` table rendered from `V::default()`.
//!
//! Helper attributes (`#[template(...)]`):
//!
//! - `env = "VAR"`: documents an overriding environment variable and
//!   contributes an env binding (only on `bool` and string-like leaves;
//!   dynamic map entries cannot be addressed by environment);
//! - `example = "..."`: TOML snippet for `Option` placeholders and map
//!   example keys (auto-quoted for `String`/`PathBuf` fields);
//! - `table`: emit a `[table]` header for a struct field;
//! - `skip`: omit the field from the template entirely.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, GenericArgument, Ident, Lit, LitStr, PathArguments,
    Token, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

#[proc_macro_derive(ConfigTemplate, attributes(template))]
pub fn derive_config_template(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[derive(Default)]
struct FieldAttr {
    table: bool,
    example: Option<String>,
    env: Option<String>,
    skip: bool,
}

struct TemplateArg {
    name: Ident,
    value: Option<LitStr>,
}

struct TemplateArgs(Vec<TemplateArg>);

impl Parse for TemplateArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Vec::new();
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            let value = if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                Some(input.parse::<LitStr>()?)
            } else {
                None
            };
            args.push(TemplateArg { name, value });
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(Self(args))
    }
}

fn parse_field_attr(attrs: &[Attribute]) -> syn::Result<FieldAttr> {
    let mut out = FieldAttr::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("template")) {
        let parsed: TemplateArgs = attr.parse_args_with(TemplateArgs::parse)?;
        for arg in parsed.0 {
            let name = arg.name.to_string();
            match name.as_str() {
                "table" => {
                    require_no_value(&arg)?;
                    out.table = true;
                }
                "skip" => {
                    require_no_value(&arg)?;
                    out.skip = true;
                }
                "example" => {
                    out.example = Some(require_value(&arg)?);
                }
                "env" => {
                    out.env = Some(require_value(&arg)?);
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        arg.name,
                        format!("unknown template argument `{other}`"),
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn require_no_value(arg: &TemplateArg) -> syn::Result<()> {
    if arg.value.is_some() {
        return Err(syn::Error::new_spanned(
            &arg.name,
            format!("`{}` takes no value", arg.name),
        ));
    }
    Ok(())
}

fn require_value(arg: &TemplateArg) -> syn::Result<String> {
    arg.value.as_ref().map(|lit| lit.value()).ok_or_else(|| {
        syn::Error::new_spanned(&arg.name, format!("`{}` requires a value", arg.name))
    })
}

/// Reads `///` doc comments as plain lines (one leading space stripped).
/// `#[doc = "..."]` is a name-value attribute, which `parse_args` cannot
/// read, so match `attr.meta` directly instead.
fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| {
            if let syn::Meta::NameValue(pair) = &attr.meta {
                if let Expr::Lit(expr) = &pair.value {
                    if let Lit::Str(lit) = &expr.lit {
                        let line = lit.value();
                        return Some(line.strip_prefix(' ').unwrap_or(&line).to_owned());
                    }
                }
            }
            None
        })
        .collect()
}

fn comment_block(lines: &[String]) -> proc_macro2::TokenStream {
    let mut tokens = proc_macro2::TokenStream::new();
    for line in lines {
        if line.is_empty() {
            tokens.extend(quote! { __out.push_str("#\n"); });
        } else {
            let lit = LitStr::new(line, Span::call_site());
            tokens.extend(quote! {
                __out.push_str("# ");
                __out.push_str(#lit);
                __out.push('\n');
            });
        }
    }
    tokens
}

fn last_segment(ty: &Type) -> Option<String> {
    if let Type::Path(path) = ty {
        path.path.segments.last().map(|seg| seg.ident.to_string())
    } else {
        None
    }
}

fn generic_type_arg(ty: &Type, index: usize) -> Option<Type> {
    if let Type::Path(path) = ty {
        let segment = path.path.segments.last()?;
        if let PathArguments::AngleBracketed(args) = &segment.arguments {
            return args
                .args
                .iter()
                .filter_map(|arg| {
                    if let GenericArgument::Type(inner) = arg {
                        Some(inner.clone())
                    } else {
                        None
                    }
                })
                .nth(index);
        }
    }
    None
}

fn is_leaf_name(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "PathBuf"
            | "bool"
            | "char"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "Vec"
    )
}

fn is_string_like(name: &str) -> bool {
    matches!(name, "String" | "PathBuf")
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "ConfigTemplate only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "ConfigTemplate can only be derived for structs",
            ));
        }
    };

    let struct_docs = comment_block(&doc_lines(&input.attrs));

    let mut body = proc_macro2::TokenStream::new();
    let mut env_body = proc_macro2::TokenStream::new();
    for field in fields {
        let ident = field.ident.as_ref().expect("named fields");
        let key = ident.to_string();
        let attr = parse_field_attr(&field.attrs)?;
        if attr.skip {
            continue;
        }
        let docs = comment_block(&doc_lines(&field.attrs));
        let env = attr.env.as_deref().map(|var| {
            let lit = LitStr::new(&format!("Env: {var}"), Span::call_site());
            quote! {
                __out.push_str("# ");
                __out.push_str(#lit);
                __out.push('\n');
            }
        });

        let ty = &field.ty;
        let (stripped, is_option) = match last_segment(ty).as_deref() {
            Some("Option") => match generic_type_arg(ty, 0) {
                Some(inner) => (inner, true),
                None => {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "Option fields need an explicit type argument",
                    ));
                }
            },
            _ => ((*ty).clone(), false),
        };
        let stripped_name = last_segment(&stripped).unwrap_or_default();

        if stripped_name == "HashMap" {
            if attr.table {
                return Err(syn::Error::new_spanned(
                    ident,
                    "`table` is only supported on struct fields",
                ));
            }
            if attr.env.is_some() {
                return Err(syn::Error::new_spanned(
                    ident,
                    "`env` needs a static key path; dynamic map entries cannot be addressed by environment",
                ));
            }
            let example = attr.example.ok_or_else(|| {
                syn::Error::new_spanned(
                    ident,
                    "HashMap fields need #[template(example = \"slug\")]",
                )
            })?;
            let value_ty = generic_type_arg(&stripped, 1).ok_or_else(|| {
                syn::Error::new_spanned(ty, "HashMap fields need a value type argument")
            })?;
            let example_lit = LitStr::new(&example, Span::call_site());
            body.extend(quote! {
                #docs
                #env
                {
                    let __key = if __path.is_empty() {
                        format!("{}.{:?}", #key, #example_lit)
                    } else {
                        format!("{}.{}.{:?}", __path, #key, #example_lit)
                    };
                    <#value_ty>::default().__marina_config_section(__out, &__key, __commented);
                }
            });
        } else if is_leaf_name(&stripped_name) {
            if attr.table {
                return Err(syn::Error::new_spanned(
                    ident,
                    "`table` is only supported on struct fields",
                ));
            }
            // `env` bindings reuse the template's path threading so the
            // merge code can never disagree with the documented key path.
            // Bools need lenient truthy parsing; everything else merges as
            // a string and lets figment coerce on extract.
            if let Some(var) = attr.env.as_deref() {
                let is_bool = stripped_name == "bool";
                if !is_bool && !is_string_like(&stripped_name) {
                    return Err(syn::Error::new_spanned(
                        ident,
                        "`env` is only supported on bool and string-like leaf fields",
                    ));
                }
                let var_lit = LitStr::new(var, Span::call_site());
                env_body.extend(quote! {
                    {
                        let __key = if __prefix.is_empty() {
                            String::from(#key)
                        } else {
                            format!("{}.{}", __prefix, #key)
                        };
                        __bindings.push((__key, #var_lit, #is_bool));
                    }
                });
            }
            if is_option {
                let mut example = attr.example.ok_or_else(|| {
                    syn::Error::new_spanned(
                        ident,
                        "Option fields need #[template(example = \"...\")]",
                    )
                })?;
                if is_string_like(&stripped_name) && !example.trim_start().starts_with('"') {
                    example = format!("\"{example}\"");
                }
                let example_lit = LitStr::new(&example, Span::call_site());
                body.extend(quote! {
                    #docs
                    #env
                    match &self.#ident {
                        ::std::option::Option::Some(__value) => {
                            __out.push_str(#key);
                            __out.push_str(" = ");
                            __out.push_str(&format!("{:?}", __value));
                            __out.push('\n');
                        }
                        ::std::option::Option::None => {
                            if __commented {
                                __out.push_str("# ");
                                __out.push_str(#key);
                                __out.push_str(" = ");
                                __out.push_str(#example_lit);
                                __out.push('\n');
                            } else {
                                __out.push_str(#key);
                                __out.push_str(" = ");
                                __out.push_str(#example_lit);
                                __out.push('\n');
                            }
                        }
                    }
                });
            } else {
                if attr.example.is_some() {
                    return Err(syn::Error::new_spanned(
                        ident,
                        "`example` is only supported on Option and HashMap fields",
                    ));
                }
                body.extend(quote! {
                    #docs
                    #env
                    __out.push_str(#key);
                    __out.push_str(" = ");
                    __out.push_str(&format!("{:?}", &self.#ident));
                    __out.push('\n');
                });
            }
        } else {
            if is_option {
                return Err(syn::Error::new_spanned(
                    ident,
                    "Option<struct> fields are not supported; use a struct field instead",
                ));
            }
            if attr.example.is_some() {
                return Err(syn::Error::new_spanned(
                    ident,
                    "`example` is only supported on Option and HashMap fields",
                ));
            }
            if attr.env.is_some() {
                return Err(syn::Error::new_spanned(
                    ident,
                    "`env` is only supported on bool and string-like leaf fields",
                ));
            }
            // Env bindings thread the same dotted path as the template.
            let env_recurse = quote! {
                {
                    let __sub = if __prefix.is_empty() {
                        String::from(#key)
                    } else {
                        format!("{}.{}", __prefix, #key)
                    };
                    __bindings.extend(self.#ident.__marina_env_bindings(&__sub));
                }
            };
            if attr.table {
                body.extend(quote! {
                    #docs
                    #env
                    {
                        let __sub = if __path.is_empty() {
                            String::from(#key)
                        } else {
                            format!("{}.{}", __path, #key)
                        };
                        self.#ident.__marina_config_section(__out, &__sub, __commented);
                    }
                });
                env_body.extend(env_recurse);
            } else {
                body.extend(quote! {
                    {
                        let __sub = if __path.is_empty() {
                            String::from(#key)
                        } else {
                            format!("{}.{}", __path, #key)
                        };
                        self.#ident.__marina_config_transparent(__out, &__sub, __commented);
                    }
                });
                env_body.extend(env_recurse);
            }
        }
    }

    Ok(quote! {
        impl #name {
            pub fn __marina_config_section(
                &self,
                __out: &mut String,
                __path: &str,
                __commented: bool,
            ) {
                #struct_docs
                __out.push('\n');
                __out.push_str(&format!("[{}]\n", __path));
                self.__marina_config_body(__out, __path, __commented);
            }

            pub fn __marina_config_transparent(
                &self,
                __out: &mut String,
                __path: &str,
                __commented: bool,
            ) {
                // Transparent namespaces only extend the table path; their
                // own docs stay in rustdoc so they cannot pile up above an
                // unrelated first header.
                self.__marina_config_body(__out, __path, __commented);
            }

            fn __marina_config_body(
                &self,
                __out: &mut String,
                __path: &str,
                __commented: bool,
            ) {
                #body
            }

            /// Environment bindings as `(dotted.path, VAR, is_bool)` tuples,
            /// collected from every `#[template(env = "..")]` field. A single
            /// hand-written applier merges them over the file layer, so the
            /// merge code can never disagree with the documented key path.
            /// `is_bool` selects lenient truthy parsing; other values merge
            /// as strings. Map entries have no static path and contribute
            /// nothing.
            pub fn __marina_env_bindings(
                &self,
                __prefix: &str,
            ) -> Vec<(String, &'static str, bool)> {
                let mut __bindings = Vec::new();
                #env_body
                __bindings
            }
        }
    })
}
