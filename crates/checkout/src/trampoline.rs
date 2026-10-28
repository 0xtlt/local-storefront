//! What `shopify app build` does to a Function's module after its build command. A Rust Function
//! calls the Function Wasm API of the host by name (`shopify_function_v2::shopify_function_input_get`),
//! and Shopify's `shopify-function-trampoline` links it to the host's provider. lsf runs that same
//! trampoline, as a library, at the version Shopify's CLI 5.0.0 runs (2.0.1), so that a module
//! `cargo build` writes runs without the CLI.

use std::path::Path;

/// Links the module at `path` to the provider, in place, when it calls the API by name; a module
/// already linked, or that does not use the API (a JavaScript Function's), is left as it is.
/// Returns whether it linked it.
pub fn apply(path: &Path) -> Result<bool, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if unlinked(&bytes).is_none() {
        return Ok(false);
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".trampoline");
    let partial = std::path::PathBuf::from(partial);
    let linked = shopify_function_trampoline::trampoline_existing_module(path, &partial)
        .map_err(|error| format!("shopify-function-trampoline: {error:#}"))
        .and_then(|()| {
            std::fs::rename(&partial, path)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))
        });
    if linked.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    linked.map(|()| true)
}

/// The first import of the module that calls the Function Wasm API by name
/// (`shopify_function_v2`, `shopify_function_input_get`): what the trampoline replaces. One it
/// added is named `_shopify_function_…`, next to the provider's `memory`.
pub fn unlinked(bytes: &[u8]) -> Option<(String, String)> {
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let Ok(wasmparser::Payload::ImportSection(imports)) = payload else {
            continue;
        };
        for import in imports.into_iter().flatten() {
            if import.module.starts_with("shopify_function_v")
                && import.name != "memory"
                && !import.name.starts_with('_')
            {
                return Some((import.module.to_string(), import.name.to_string()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(path)
    }

    fn temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lsf-trampoline-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("function.wasm");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_module_cargo_built_is_linked_as_shopify_s_trampoline_links_it() {
        let raw = std::fs::read(fixture("trampoline/quantity-limit.raw.wasm")).unwrap();
        assert_eq!(
            unlinked(&raw),
            Some((
                "shopify_function_v2".to_string(),
                "shopify_function_input_get_obj_prop".to_string()
            ))
        );
        let path = temp("raw", &raw);
        assert!(apply(&path).unwrap());
        let linked = std::fs::read(&path).unwrap();
        // What shopify-function-trampoline 2.0.1, the binary, writes.
        assert_eq!(
            linked,
            std::fs::read(fixture("app/extensions/quantity-limit/dist/function.wasm")).unwrap()
        );
        assert_eq!(unlinked(&linked), None);
        // Again: it is linked already, and stays as it is.
        assert!(!apply(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), linked);
    }

    #[test]
    fn a_module_that_does_not_call_the_api_by_name_is_left_as_it_is() {
        let javascript = std::fs::read(fixture("javy/discount.wasm")).unwrap();
        let path = temp("javy", &javascript);
        assert!(!apply(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), javascript);
    }
}
