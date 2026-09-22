//! `#[derive(ConfigTemplate)]`: renders a commented TOML template of a
//! config struct's defaults, using `///` doc comments as documentation.
//! `#[derive(ConfigSettings)]` flattens the same configuration tree into
//! metadata rows suitable for a settings panel.
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

use std::collections::HashMap;

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, GenericArgument, Ident, Lit, LitStr, PathArguments,
    Token, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

#[proc_macro_derive(ConfigTemplate, attributes(template, setting))]
pub fn derive_config_template(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(ConfigSettings, attributes(setting, template))]
pub fn derive_config_settings(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_settings(&input) {
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

#[derive(Default)]
struct SettingsAttr {
    title: Option<String>,
    control: Option<String>,
    sensitive: bool,
    panel: Option<String>,
    panel_title: Option<String>,
    panel_order: Option<i32>,
    section: Option<String>,
    section_title: Option<String>,
    section_order: Option<i32>,
    order: Option<i32>,
    skip: bool,
}

#[derive(Default)]
struct SectionMeta {
    title: Option<String>,
    order: Option<i32>,
}

#[derive(Default)]
struct PanelMeta {
    title: Option<String>,
    order: Option<i32>,
}

fn parse_settings_attr(attrs: &[Attribute]) -> syn::Result<SettingsAttr> {
    let mut out = SettingsAttr::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("setting")) {
        let parsed: TemplateArgs = attr.parse_args_with(TemplateArgs::parse)?;
        for arg in parsed.0 {
            let name = arg.name.to_string();
            match name.as_str() {
                "title" => out.title = Some(require_value(&arg)?),
                "control" => out.control = Some(require_value(&arg)?),
                "sensitive" => {
                    require_no_value(&arg)?;
                    out.sensitive = true;
                }
                "panel" => out.panel = Some(require_value(&arg)?),
                "panel_title" => out.panel_title = Some(require_value(&arg)?),
                "panel_order" => out.panel_order = Some(parse_order(&arg)?),
                "section" => out.section = Some(require_value(&arg)?),
                "section_title" => out.section_title = Some(require_value(&arg)?),
                "section_order" => out.section_order = Some(parse_order(&arg)?),
                "order" => out.order = Some(parse_order(&arg)?),
                "skip" | "hidden" => {
                    require_no_value(&arg)?;
                    out.skip = true;
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        arg.name,
                        format!("unknown setting argument `{other}`"),
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn parse_order(arg: &TemplateArg) -> syn::Result<i32> {
    let value = require_value(arg)?;
    value.parse::<i32>().map_err(|_| {
        syn::Error::new_spanned(
            &arg.name,
            format!("`{}` must be a signed 32-bit integer", arg.name),
        )
    })
}

fn human_title(name: &str) -> String {
    let mut title = String::new();
    for (index, ch) in name.chars().enumerate() {
        if index == 0 {
            title.extend(ch.to_uppercase());
        } else if ch == '_' || ch == '-' {
            title.push(' ');
        } else {
            title.push(ch);
        }
    }
    title
}

fn setting_env_tokens(attr: &FieldAttr, ident: &Ident) -> proc_macro2::TokenStream {
    match attr.env.as_deref() {
        Some(value) => {
            let value = LitStr::new(value, ident.span());
            quote! { Some(#value) }
        }
        None => quote! { None },
    }
}

fn setting_control(name: &str) -> &'static str {
    match name {
        "bool" => "toggle",
        "String" | "PathBuf" => "text",
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" => "number",
        "Vec" => "list",
        "HashMap" => "map",
        _ => "text",
    }
}

fn settings_type(ty: &Type) -> (Type, bool) {
    match last_segment(ty).as_deref() {
        Some("Option") => generic_type_arg(ty, 0)
            .map(|inner| (inner, true))
            .unwrap_or_else(|| (ty.clone(), false)),
        _ => (ty.clone(), false),
    }
}

fn settings_path(prefix: proc_macro2::TokenStream, key: &LitStr) -> proc_macro2::TokenStream {
    quote! {
        if #prefix.is_empty() {
            String::from(#key)
        } else {
            format!("{}.{}", #prefix, #key)
        }
    }
}

fn expand_settings(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "ConfigSettings only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "ConfigSettings can only be derived for structs",
            ));
        }
    };

    // Register section metadata before expanding any field so sibling fields can
    // reference a section by id without repeating its title or order.
    let mut section_registry = HashMap::<String, SectionMeta>::new();
    let mut panel_registry = HashMap::<String, PanelMeta>::new();
    for field in fields {
        let setting = parse_settings_attr(&field.attrs)?;
        if let Some(section) = setting.section.as_ref() {
            let metadata = section_registry.entry(section.clone()).or_default();
            if let Some(title) = setting.section_title {
                metadata.title = Some(title);
            }
            if let Some(order) = setting.section_order {
                metadata.order = Some(order);
            }
        }
        if let Some(panel) = setting.panel.as_ref() {
            let metadata = panel_registry.entry(panel.clone()).or_default();
            if let Some(title) = setting.panel_title {
                metadata.title = Some(title);
            }
            if let Some(order) = setting.panel_order {
                metadata.order = Some(order);
            }
        }
    }

    let mut body = proc_macro2::TokenStream::new();
    for field in fields {
        let ident = field.ident.as_ref().expect("named fields");
        let field_name = ident.to_string();
        let key = LitStr::new(&field_name, ident.span());
        let template = parse_field_attr(&field.attrs)?;
        let setting = parse_settings_attr(&field.attrs)?;
        if template.skip || setting.skip {
            continue;
        }
        if last_segment(&field.ty).as_deref() == Some("Option")
            && generic_type_arg(&field.ty, 0).is_none()
        {
            return Err(syn::Error::new_spanned(
                &field.ty,
                "Option fields need an explicit type argument",
            ));
        }
        let (inner_ty, is_option) = settings_type(&field.ty);
        let type_name = last_segment(&inner_ty).unwrap_or_default();
        let docs = doc_lines(&field.attrs).join("\n");
        let title_text = setting
            .title
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| human_title(&field_name));
        let title = LitStr::new(&title_text, ident.span());
        let control = setting
            .control
            .as_deref()
            .map(|value| LitStr::new(value, ident.span()))
            .unwrap_or_else(|| LitStr::new(setting_control(&type_name), ident.span()));
        let description = LitStr::new(&docs, ident.span());
        let env = setting_env_tokens(&template, ident);
        let panel = setting
            .panel
            .as_deref()
            .map(|value| LitStr::new(value, ident.span()));
        let registered_panel = setting
            .panel
            .as_ref()
            .and_then(|panel| panel_registry.get(panel));
        let resolved_panel_title = setting
            .panel_title
            .clone()
            .or_else(|| registered_panel.and_then(|metadata| metadata.title.clone()));
        let resolved_panel_order = setting
            .panel_order
            .or_else(|| registered_panel.and_then(|metadata| metadata.order));
        let panel_tokens = match panel.as_ref() {
            Some(value) => quote! { String::from(#value) },
            None => quote! { __panel.to_owned() },
        };
        let panel_title = resolved_panel_title
            .as_deref()
            .map(|value| LitStr::new(value, ident.span()));
        let panel_title_tokens = match (panel.as_ref(), panel_title.as_ref()) {
            (Some(_panel), Some(title)) => quote! { String::from(#title) },
            (Some(panel), None) => {
                let title = LitStr::new(&human_title(&panel.value()), ident.span());
                quote! { String::from(#title) }
            }
            (None, Some(title)) => quote! { String::from(#title) },
            (None, None) => quote! { __panel_title.to_owned() },
        };
        let panel_order = resolved_panel_order
            .map(|value| quote! { #value })
            .unwrap_or_else(|| quote! { __panel_order });
        let sensitive = setting.sensitive;
        let path = settings_path(quote! { __prefix }, &key);
        let section = setting
            .section
            .as_deref()
            .map(|value| LitStr::new(value, ident.span()));
        let registered_section = setting
            .section
            .as_ref()
            .and_then(|section| section_registry.get(section));
        let resolved_section_title = setting
            .section_title
            .clone()
            .or_else(|| registered_section.and_then(|metadata| metadata.title.clone()));
        let resolved_section_order = setting
            .section_order
            .or_else(|| registered_section.and_then(|metadata| metadata.order));
        let section_title = resolved_section_title
            .as_deref()
            .map(|value| LitStr::new(value, ident.span()));
        let section_tokens = match section.as_ref() {
            Some(value) => quote! { String::from(#value) },
            None => quote! { __section.to_owned() },
        };
        let section_title_tokens = match (section.as_ref(), section_title.as_ref()) {
            (Some(_section), Some(title)) => quote! { String::from(#title) },
            (Some(section), None) => {
                let title = LitStr::new(&human_title(&section.value()), ident.span());
                quote! { String::from(#title) }
            }
            (None, Some(title)) => quote! { String::from(#title) },
            (None, None) => quote! { __section_title.to_owned() },
        };
        let section_order = resolved_section_order
            .map(|value| quote! { #value })
            .unwrap_or_else(|| quote! { __section_order });
        let order = setting.order.unwrap_or(0);

        if type_name == "HashMap" {
            body.extend(quote! {
                __out.push((#path, String::from(#title), String::from(#description), #control, #env, #sensitive, #panel_tokens, #panel_title_tokens, #panel_order, #section_tokens, #section_title_tokens, #section_order, #order));
            });
        } else if is_leaf_name(&type_name) {
            body.extend(quote! {
                __out.push((#path, String::from(#title), String::from(#description), #control, #env, #sensitive, #panel_tokens, #panel_title_tokens, #panel_order, #section_tokens, #section_title_tokens, #section_order, #order));
            });
        } else if is_option {
            return Err(syn::Error::new_spanned(
                &field.ty,
                "ConfigSettings only supports Option<leaf> and Option<HashMap<...>> fields",
            ));
        } else {
            let child_panel = match panel.as_ref() {
                Some(value) => quote! { #value },
                None => quote! { __panel },
            };
            let child_panel_title = match (panel.as_ref(), panel_title.as_ref()) {
                (_, Some(title)) => quote! { #title },
                (Some(panel), None) => {
                    let title = LitStr::new(&human_title(&panel.value()), ident.span());
                    quote! { #title }
                }
                (None, None) => quote! { __panel_title },
            };
            let child_panel_order = resolved_panel_order
                .map(|value| quote! { #value })
                .unwrap_or_else(|| quote! { __panel_order });
            let child_section = match section.as_ref() {
                Some(value) => quote! { #value },
                None => quote! { #key },
            };
            let child_section_title = match (section.as_ref(), section_title.as_ref()) {
                (_, Some(title)) => quote! { #title },
                (Some(section), None) => {
                    let title = LitStr::new(&human_title(&section.value()), ident.span());
                    quote! { #title }
                }
                (None, None) => {
                    let title = LitStr::new(&human_title(&field_name), ident.span());
                    quote! { #title }
                }
            };
            let child_section_order = resolved_section_order
                .map(|value| quote! { #value })
                .unwrap_or_else(|| quote! { __section_order });
            body.extend(quote! {
                __out.extend(self.#ident.__marina_settings_with_section(
                    &#path,
                    #child_panel,
                    #child_panel_title,
                    #child_panel_order,
                    #child_section,
                    #child_section_title,
                    #child_section_order,
                ));
            });
        }
    }

    Ok(quote! {
        impl #name {
            pub fn __marina_settings(
                &self,
                __prefix: &str,
            ) -> Vec<(String, String, String, &'static str, Option<&'static str>, bool, String, String, i32, String, String, i32, i32)> {
                self.__marina_settings_with_section(__prefix, "", "", 0, "", "", 0)
            }

            pub fn __marina_settings_with_section(
                &self,
                __prefix: &str,
                __panel: &str,
                __panel_title: &str,
                __panel_order: i32,
                __section: &str,
                __section_title: &str,
                __section_order: i32,
            ) -> Vec<(String, String, String, &'static str, Option<&'static str>, bool, String, String, i32, String, String, i32, i32)> {
                let mut __out = Vec::new();
                #body
                __out
            }
        }
    })
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
