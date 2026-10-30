//! The Wasm module of a JavaScript Function, as Javy writes it when Shopify's CLI builds one
//! (`javy build -C dynamic -C plugin=shopify_functions_javy_v4.wasm -C wit=… -C wit-world=…`).
//!
//! The module holds no JavaScript engine. It holds the QuickJS bytecode of the Function, which
//! Shopify's Javy plugin compiles (its `compile-src`), and imports the engine from that plugin,
//! which whoever runs the Function links: Shopify, and [`crate::runner`] here. Each export
//! allocates the bytecode in the plugin's memory and asks the plugin to run it (`invoke`), then
//! to call the JavaScript function of the same name.
//!
//! For the same JavaScript, the bytes are the ones Javy 9.1.0 writes, which Shopify's CLI 4.9.5
//! runs (its `wasm-opt` pass changes nothing in such a module): a test checks it.

use std::io::Cursor;
use std::sync::OnceLock;

use wasm_encoder::{
    CodeSection, CustomSection, DataCountSection, DataSection, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, MemoryType, Module as Encoder,
    ProducersField, ProducersSection, TypeSection, ValType,
};
use wasmtime::{Cache, Config, Engine, Instance, Module, Store};

/// The module Shopify's Javy plugin, version 4, is imported from (its `import_namespace` custom
/// section): QuickJS and the `ShopifyFunction` global the Functions read their input from. lsf
/// compiles with the provider it runs the modules with ([`crate::providers`]), so that a module
/// runs on the plugin that compiled it.
const PLUGIN_NAMESPACE: &str = "shopify_functions_javy_v4";

/// The Javy the producers section names: the module is the one it writes.
const JAVY_VERSION: &str = "9.1.0";

/// The module for `js`, an ES module, with an export for each name of `exports`, as written in
/// `shopify.extension.toml` (kebab-case): it calls the function the module exports under the
/// same name in camelCase. `_start` runs the module's top level only.
pub fn module(js: &str, exports: &[&str]) -> Result<Vec<u8>, String> {
    let bytecode = compile(js)?;
    let length = i32::try_from(bytecode.len())
        .map_err(|_| "the bytecode of the Function is too large".to_string())?;
    let names: Vec<String> = exports.iter().map(|export| camel_case(export)).collect();
    let count = u32::try_from(exports.len()).expect("a few exports");

    let mut module = Encoder::new();

    // 0: the exports, 1: `cabi_realloc`, 2: `invoke`.
    let mut types = TypeSection::new();
    types.ty().function([], []);
    types.ty().function([ValType::I32; 4], [ValType::I32]);
    types.ty().function([ValType::I32; 5], []);
    module.section(&types);

    // Functions 0 and 1, and memory 0.
    let mut imports = ImportSection::new();
    imports.import(PLUGIN_NAMESPACE, "cabi_realloc", EntityType::Function(1));
    imports.import(PLUGIN_NAMESPACE, "invoke", EntityType::Function(2));
    imports.import(
        PLUGIN_NAMESPACE,
        "memory",
        MemoryType {
            minimum: 0,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        },
    );
    module.section(&imports);

    // The exports first, then `_start`, as Javy orders them.
    let mut functions = FunctionSection::new();
    for _ in 0..=count {
        functions.function(0);
    }
    module.section(&functions);

    let mut export_section = ExportSection::new();
    export_section.export("_start", ExportKind::Func, 2 + count);
    for (index, export) in (0..).zip(exports) {
        export_section.export(export, ExportKind::Func, 2 + index);
    }
    module.section(&export_section);

    // Data 0 is the bytecode, then the name of each export's function.
    module.section(&DataCountSection { count: 1 + count });

    let mut code = CodeSection::new();
    for (index, name) in (1..).zip(&names) {
        let name_length = i32::try_from(name.len()).expect("a short name");
        // Local 0: where the bytecode is; local 1: where the name is.
        let mut function = Function::new([(2, ValType::I32)]);
        function
            .instructions()
            .i32_const(0)
            .i32_const(0)
            .i32_const(1)
            .i32_const(length)
            .call(0)
            .local_tee(0)
            .i32_const(0)
            .i32_const(length)
            .memory_init(0, 0)
            .data_drop(0)
            .i32_const(0)
            .i32_const(0)
            .i32_const(1)
            .i32_const(name_length)
            .call(0)
            .local_tee(1)
            .i32_const(0)
            .i32_const(name_length)
            .memory_init(0, index)
            .data_drop(index)
            .local_get(0)
            .i32_const(length)
            .i32_const(1)
            .local_get(1)
            .i32_const(name_length)
            .call(1)
            .end();
        code.function(&function);
    }
    let mut start = Function::new([(1, ValType::I32)]);
    start
        .instructions()
        .i32_const(0)
        .i32_const(0)
        .i32_const(1)
        .i32_const(length)
        .call(0)
        .local_tee(0)
        .i32_const(0)
        .i32_const(length)
        .memory_init(0, 0)
        .local_get(0)
        .i32_const(length)
        .i32_const(0)
        .i32_const(0)
        .i32_const(0)
        .call(1)
        .end();
    code.function(&start);
    module.section(&code);

    let mut data = DataSection::new();
    data.passive(bytecode);
    for name in &names {
        data.passive(name.bytes());
    }
    module.section(&data);

    let mut producers = ProducersSection::new();
    let mut language = ProducersField::new();
    language.value("JavaScript", "ES2020");
    producers.field("language", &language);
    let mut processed_by = ProducersField::new();
    processed_by.value("Javy", JAVY_VERSION);
    producers.field("processed-by", &processed_by);
    module.section(&producers);

    module.section(&CustomSection {
        name: "javy_source".into(),
        data: compress(js).into(),
    });

    Ok(module.finish())
}

/// The name of the JavaScript function an export calls: `cart-lines-discounts-generate-run`
/// calls `cartLinesDiscountsGenerateRun`.
pub fn camel_case(export: &str) -> String {
    let mut name = String::with_capacity(export.len());
    for (index, word) in export
        .split('-')
        .filter(|word| !word.is_empty())
        .enumerate()
    {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                name.extend(first.to_lowercase());
            } else {
                name.extend(first.to_uppercase());
            }
            name.extend(chars.flat_map(char::to_lowercase));
        }
    }
    name
}

/// The QuickJS bytecode of `js`, from the plugin's `compile-src`. The plugin traps on a syntax
/// error (Javy's CLI too) instead of saying what it is: the bundler, which parses every module
/// first, reports those, as esbuild does before Javy runs.
fn compile(js: &str) -> Result<Vec<u8>, String> {
    let (engine, plugin) = plugin()?;
    let failed = |error: wasmtime::Error| {
        let reason = match error.downcast_ref::<wasmtime::Trap>() {
            Some(trap) => trap.to_string(),
            None => error.to_string(),
        };
        format!("the Javy plugin could not compile the JavaScript: {reason}")
    };
    let mut store = Store::new(engine, ());
    let instance = Instance::new(&mut store, plugin, &[]).map_err(failed)?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| "the Javy plugin has no memory".to_string())?;
    let realloc = instance
        .get_typed_func::<(u32, u32, u32, u32), u32>(&mut store, "cabi_realloc")
        .map_err(failed)?;
    let compile = instance
        .get_typed_func::<(u32, u32), u32>(&mut store, "compile-src")
        .map_err(failed)?;

    let length = u32::try_from(js.len()).map_err(|_| "the JavaScript is too large".to_string())?;
    let source = realloc
        .call(&mut store, (0, 0, 1, length))
        .map_err(failed)?;
    memory
        .write(&mut store, source as usize, js.as_bytes())
        .map_err(|error| failed(error.into()))?;
    let result = compile.call(&mut store, (source, length)).map_err(failed)?;

    // A `result<list<u8>, string>`: the variant, then where the bytes are and how many.
    let mut header = [0; 12];
    memory
        .read(&store, result as usize, &mut header)
        .map_err(|error| failed(error.into()))?;
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes"));
    let mut bytes = vec![0; word(8) as usize];
    memory
        .read(&store, word(4) as usize, &mut bytes)
        .map_err(|error| failed(error.into()))?;
    if word(0) != 0 {
        return Err(format!(
            "the JavaScript does not compile: {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    Ok(bytes)
}

/// The engine and the compiled plugin, once per process. Compiled modules go to wasmtime's cache
/// on disk, as the runs' do, so that a build does not compile the plugin again; where
/// that cache cannot be set up, the plugin is compiled without it.
fn plugin() -> Result<&'static (Engine, Module), String> {
    static PLUGIN_MODULE: OnceLock<Result<(Engine, Module), String>> = OnceLock::new();
    PLUGIN_MODULE
        .get_or_init(|| {
            let engine = Cache::from_file(None)
                .ok()
                .and_then(|cache| {
                    let mut config = Config::new();
                    config.cache(Some(cache));
                    Engine::new(&config).ok()
                })
                .unwrap_or_default();
            let plugin = crate::providers::get(PLUGIN_NAMESPACE)
                .ok_or_else(|| format!("lsf has no {PLUGIN_NAMESPACE}"))?;
            let module = Module::new(&engine, plugin)
                .map_err(|error| format!("cannot load the Javy plugin: {error:#}"))?;
            Ok((engine, module))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// The source as Javy embeds it (`-C source=compressed`, its default): Brotli, at quality 11.
fn compress(js: &str) -> Vec<u8> {
    let mut compressed = Vec::new();
    brotli::enc::BrotliCompress(
        &mut Cursor::new(js.as_bytes()),
        &mut compressed,
        &brotli::enc::BrotliEncoderParams {
            quality: 11,
            ..Default::default()
        },
    )
    .expect("compressing to memory does not fail");
    compressed
}

#[cfg(test)]
mod tests;
