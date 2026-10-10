use proc_macro::TokenStream;

/// `ApplyJsonSchema` from [json-schema](https://github.com/my-jet-tools/json-schema),
/// with the generated code pointing at `mcp_server_middleware::json_schema`,
/// so a crate which uses the middleware needs no `json-schema` dependency of its own.
#[proc_macro_derive(ApplyJsonSchema, attributes(property))]
pub fn apply_json_schema(input: TokenStream) -> TokenStream {
    let ast = syn::parse_macro_input!(input as syn::DeriveInput);
    match json_schema_macros_core::generate(
        &ast,
        &quote::quote!(::mcp_server_middleware::json_schema),
    ) {
        Ok(result) => result.into(),
        Err(err) => err.into_compile_error().into(),
    }
}
