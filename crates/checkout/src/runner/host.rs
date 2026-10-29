//! The host of a run: what Shopify's function-runner v9.2.2 (Apache-2.0) does to run a module
//! (`engine::run` and what it calls), ported to run the way it runs, with the parts lsf uses
//! only. The input goes in as JSON, or as MessagePack to a module that imports a provider that
//! reads it; the module reads it with WASI (stdin, stdout, stderr, all deterministic) or from the
//! memory of the provider it imports, which the host links ([`crate::providers`]); every Wasm
//! instruction costs one unit of fuel.
//!
//! function-runner as a library brought what its command line uses with it: every compiler and
//! architecture of wasmtime, its profiler, its GraphQL validator, and its providers, 9.7 MB of
//! them uncompressed.

use std::collections::HashMap;

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use serde_json::Value;
use wasmtime::{
    AsContext, AsContextMut, Cache, Config, Engine, Instance, Linker, Module, ResourceLimiter,
    Store,
};
use wasmtime_wasi::I32Exit;
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

const STARTING_FUEL: u64 = u64::MAX;

/// One for the module, one for the memory of Javy's provider.
const MAXIMUM_MEMORIES: usize = 2;

/// Logs past this are cut in production.
const FUNCTION_LOG_LIMIT: usize = 1_000;

/// What came out of a run.
pub(super) struct Run {
    /// Wasm instructions executed.
    pub instructions: u64,
    /// What the Function logged, then why it failed if it did.
    pub logs: String,
    pub success: bool,
    /// The output, or why it does not parse.
    pub output: Result<Value, String>,
}

/// The engine of the runs: function-runner's configuration, with wasmtime's compilation cache
/// on disk if it can be set up.
pub(super) fn engine() -> wasmtime::Result<Engine> {
    let mut config = config();
    config.cache(Some(Cache::from_file(None)?));
    Engine::new(&config)
}

/// The configuration of [`engine`], without the cache.
pub(super) fn config() -> Config {
    let mut config = Config::new();
    config
        .wasm_multi_memory(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    config
}

/// Whether the input goes to the module as MessagePack: it imports the Function Wasm API, or a
/// Javy plugin of version 2 or later.
pub(super) fn uses_msgpack_provider(module: &Module) -> bool {
    module.imports().map(|i| i.module()).any(|module| {
        module.starts_with("shopify_function_v")
            || module
                .strip_prefix("shopify_functions_javy_v")
                .is_some_and(|v| v.parse::<usize>().is_ok_and(|v| v >= 2))
    })
}

/// The input of a run as the module reads it.
pub(super) fn encode_input(input: &Value, msgpack: bool) -> Result<Vec<u8>, String> {
    if msgpack {
        rmp_serde::to_vec(&MessagePackJsonValue(input))
            .map_err(|e| format!("Couldn't convert JSON to MessagePack: {e}"))
    } else {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, ShopifyJsonFormatter);
        input
            .serialize(&mut serializer)
            .map_err(|e| format!("Couldn't serialize JSON: {e}"))?;
        Ok(bytes)
    }
}

/// Runs `export` of `module` with `input` (as [`encode_input`] writes it). The compiled
/// providers are kept in `providers`, for the next runs on the same engine.
pub(super) fn run(
    engine: &Engine,
    module: &Module,
    providers: &mut HashMap<String, Module>,
    export: &str,
    input: Vec<u8>,
    msgpack: bool,
) -> wasmtime::Result<Run> {
    let provider = std_import(module)?;
    let mut io = if provider.as_ref().is_some_and(|p| p.mem_io) {
        Io::Memory(None)
    } else {
        Io::Wasi {
            output: MemoryOutputPipe::new(usize::MAX),
            logs: MemoryOutputPipe::new(usize::MAX),
        }
    };

    let mut linker = Linker::new(engine);
    let wasi = match &io {
        Io::Wasi { output, logs } => {
            let mut builder = WasiCtxBuilder::new();
            builder.stdin(MemoryInputPipe::new(input.clone()));
            builder.stdout(output.clone());
            builder.stderr(logs.clone());
            deterministic_wasi_ctx::add_determinism_to_wasi_ctx_builder(&mut builder);
            Some(builder.build_p1())
        }
        Io::Memory(_) => None,
    };
    if wasi.is_some() {
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx: &mut Context| {
            ctx.wasi.as_mut().expect("Should have WASI context")
        })?;
        deterministic_wasi_ctx::replace_scheduling_functions(&mut linker)?;
    }

    let mut store = Store::new(
        engine,
        Context {
            wasi,
            limiter: MemoryLimiter,
        },
    );
    store.limiter(|s| &mut s.limiter);

    // The provider is linked, and given the input, with fuel that does not count.
    store.set_epoch_deadline(1);
    store.set_fuel(u64::MAX)?;
    if let Some(provider) = &provider {
        let compiled = match providers.get(&provider.name) {
            Some(compiled) => compiled.clone(),
            None => {
                let compiled = Module::from_binary(engine, provider.bytes).map_err(|error| {
                    error.context(format!("Failed to load module {}", provider.name))
                })?;
                providers.insert(provider.name.clone(), compiled.clone());
                compiled
            }
        };
        let instance = linker
            .instantiate(&mut store, &compiled)
            .map_err(|error| error.context("Failed to instantiate imported instance"))?;
        linker
            .instance(&mut store, &provider.name, instance)
            .map_err(|error| error.context("Failed to import module"))?;
        if let Io::Memory(slot) = &mut io {
            let input_offset = instance
                .get_typed_func::<i32, i32>(store.as_context_mut(), "initialize")?
                .call(store.as_context_mut(), input.len() as _)?;
            memory(&instance, &mut store)?.write(
                store.as_context_mut(),
                input_offset as _,
                &input,
            )?;
            *slot = Some(instance);
        }
    }

    store.set_fuel(STARTING_FUEL)?;
    store.set_epoch_deadline(1);

    let instance = linker.instantiate(&mut store, module)?;
    let func = instance.get_typed_func::<(), ()>(store.as_context_mut(), export)?;
    let result = func.call(store.as_context_mut(), ());

    // A module may exit: with 0, it succeeded.
    let result = result.or_else(|error| match error.downcast_ref::<I32Exit>() {
        Some(I32Exit(0)) => Ok(()),
        Some(I32Exit(code)) => Err(wasmtime::Error::msg(format!(
            "module exited with code: {code}"
        ))),
        None => Err(error),
    });
    let instructions = STARTING_FUEL.saturating_sub(store.get_fuel().unwrap_or_default());

    let (output, mut logs) = match io {
        Io::Memory(instance) => {
            let instance = instance.expect("Should have been defined in initialize");
            store.set_epoch_deadline(1);
            let old_fuel = store.get_fuel()?;
            store.set_fuel(u64::MAX)?;
            let results_offset = instance
                .get_typed_func::<(), i32>(store.as_context_mut(), "finalize")?
                .call(store.as_context_mut(), ())? as usize;
            store.set_fuel(old_fuel)?;
            let memory = memory(&instance, &mut store)?;
            let read = |offset: usize, length: usize| -> wasmtime::Result<Vec<u8>> {
                let mut bytes = vec![0; length];
                memory.read(store.as_context(), offset, &mut bytes)?;
                Ok(bytes)
            };
            let header = read(results_offset, 24)?;
            let field = |at: usize| {
                u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes")) as usize
            };
            let output = read(field(0), field(4))?;
            let mut logs = read(field(8), field(12))?;
            logs.append(&mut read(field(16), field(20))?);
            if logs.len() > FUNCTION_LOG_LIMIT {
                logs.splice(0..1, b"[TRUNCATED]...".iter().copied());
            }
            (output, logs)
        }
        Io::Wasi { output, logs } => {
            // The pipes are the store's too, until it goes.
            drop(store);
            let output = output.try_into_inner().expect("one reference").to_vec();
            let logs = logs.try_into_inner().expect("one reference").to_vec();
            (output, logs)
        }
    };
    if let Err(error) = &result {
        logs.extend_from_slice(error.to_string().as_bytes());
    }

    let output = if msgpack {
        rmp_serde::decode::from_slice::<Value>(&output).map_err(|error| error.to_string())
    } else {
        serde_json::from_slice::<Value>(&output).map_err(|error| error.to_string())
    };
    Ok(Run {
        instructions,
        logs: String::from_utf8_lossy(&logs).into(),
        success: result.is_ok(),
        output,
    })
}

enum Io {
    Wasi {
        output: MemoryOutputPipe,
        logs: MemoryOutputPipe,
    },
    /// The provider's instance, once linked.
    Memory(Option<Instance>),
}

struct Context {
    wasi: Option<WasiP1Ctx>,
    limiter: MemoryLimiter,
}

struct MemoryLimiter;

impl ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }

    fn memories(&self) -> usize {
        MAXIMUM_MEMORIES
    }
}

fn memory(instance: &Instance, store: &mut Store<Context>) -> wasmtime::Result<wasmtime::Memory> {
    instance
        .get_memory(store.as_context_mut(), "memory")
        .ok_or_else(|| wasmtime::Error::msg("Missing memory export named memory"))
}

/// The provider a module imports.
struct Provider {
    name: String,
    bytes: &'static [u8],
    /// The module reads its input from the provider's memory, and writes its output there.
    mem_io: bool,
}

/// The first provider among the imports of `module`. A module that reads its input from the
/// provider's memory cannot import WASI.
fn std_import(module: &Module) -> wasmtime::Result<Option<Provider>> {
    // In order, without a hash: the first one is linked.
    let mut imports: Vec<&str> = Vec::new();
    for import in module.imports().map(|i| i.module()) {
        if !imports.contains(&import) {
            imports.push(import);
        }
    }
    let uses_wasi = imports.contains(&"wasi_snapshot_preview1");
    let provider = imports.iter().find_map(|import| {
        crate::providers::get(import).map(|bytes| {
            let version = |prefix: &str| {
                import
                    .strip_prefix(prefix)
                    .and_then(|v| v.parse::<usize>().ok())
            };
            let mem_io = version("shopify_functions_javy_v").is_some_and(|v| v >= 3)
                || version("shopify_function_v").is_some_and(|v| v >= 2);
            Provider {
                name: import.to_string(),
                bytes,
                mem_io,
            }
        })
    });
    if let Some(provider) = &provider
        && provider.mem_io
        && uses_wasi
    {
        return Err(wasmtime::Error::msg(format!(
            "Invalid Function, cannot use `{}` and import WASI. If using Rust, change the build \
             target to `wasm32-unknown-unknown`.",
            provider.name
        )));
    }
    Ok(provider)
}

/// A JSON value as MessagePack: integers as integers, other numbers as floats.
struct MessagePackJsonValue<'a>(&'a Value);

impl Serialize for MessagePackJsonValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(value) => serializer.serialize_bool(*value),
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    serializer.serialize_i64(value)
                } else if let Some(value) = value.as_u64() {
                    serializer.serialize_u64(value)
                } else if let Some(value) = value.as_f64() {
                    serializer.serialize_f64(value)
                } else {
                    serializer.serialize_str(&value.to_string())
                }
            }
            Value::String(value) => serializer.serialize_str(value),
            Value::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&MessagePackJsonValue(value))?;
                }
                seq.end()
            }
            Value::Object(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, &MessagePackJsonValue(value))?;
                }
                map.end()
            }
        }
    }
}

/// JSON as Shopify writes it: `/`, U+2028 and U+2029 escaped.
struct ShopifyJsonFormatter;

impl serde_json::ser::Formatter for ShopifyJsonFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        let mut start = 0;
        for (index, character) in fragment.char_indices() {
            let escaped: &[u8] = match character {
                '/' => br"\/",
                '\u{2028}' => br"\u2028",
                '\u{2029}' => br"\u2029",
                _ => continue,
            };
            writer.write_all(&fragment.as_bytes()[start..index])?;
            writer.write_all(escaped)?;
            start = index + character.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_input_is_json_as_shopify_writes_it() {
        let input = json!({
            "id": "gid://shopify/Product/1",
            "line": "before\u{2028}after\u{2029}",
            "longitude": serde_json::from_str::<Value>("-0.9191460999999999").unwrap(),
        });
        assert_eq!(
            String::from_utf8(encode_input(&input, false).unwrap()).unwrap(),
            r#"{"id":"gid:\/\/shopify\/Product\/1","line":"before\u2028after\u2029","longitude":-0.9191460999999999}"#
        );
    }

    #[test]
    fn the_input_is_messagepack_with_integers_and_floats() {
        let input = json!({"a": 1, "b": -2, "c": 2.5, "d": [true, null, "x"]});
        let bytes = encode_input(&input, true).unwrap();
        assert_eq!(
            bytes,
            [
                0x84, 0xa1, b'a', 0x01, 0xa1, b'b', 0xfe, 0xa1, b'c', 0xcb, 0x40, 0x04, 0, 0, 0, 0,
                0, 0, 0xa1, b'd', 0x93, 0xc3, 0xc0, 0xa1, b'x'
            ]
        );
        assert_eq!(
            rmp_serde::decode::from_slice::<Value>(&bytes).unwrap(),
            input
        );
    }
}
