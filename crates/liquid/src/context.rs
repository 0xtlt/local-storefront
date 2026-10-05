//! The render-time state: variable scopes, registers, interrupts and error collection.

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::environment::Environment;
use crate::error::{Error, ErrorKind, Result};
use crate::profiler::Profiler;
use crate::template::Template;
use crate::value::{Object, Value};
use crate::variable::FilterArgs;

/// The maximum nesting of scopes and partials, as in the reference implementation.
pub const MAX_DEPTH: usize = 100;

/// Loads the templates referenced by `render` and `include`.
pub trait PartialLoader: Send + Sync {
    fn load(&self, name: &str) -> Result<Arc<Template>>;
}

struct NoPartials;

impl PartialLoader for NoPartials {
    fn load(&self, _name: &str) -> Result<Arc<Template>> {
        Err(Error::file_system(
            "This liquid context does not allow includes.",
        ))
    }
}

struct NoGlobals;

impl Object for NoGlobals {
    fn type_name(&self) -> &str {
        "globals"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The `self` object: the variables of the scope it was read in, so that `self[name]` and
/// `self.name` read the variable `name`.
///
/// The context it belongs to answers its lookups, which is how it sees variables assigned after
/// it was read. Handed to another context (an argument of `render`) or to a filter, it takes
/// the variables along as they are at that point; its own context cannot change them until the
/// partial or the filter is done.
pub struct SelfDrop {
    scope: u64,
    captured: Option<Captured>,
}

struct Captured {
    variables: HashMap<String, Value>,
    inherited: Arc<HashMap<String, Value>>,
    globals: Arc<dyn Object>,
}

impl Object for SelfDrop {
    fn type_name(&self) -> &str {
        "self"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let captured = self.captured.as_ref()?;
        captured
            .variables
            .get(key)
            .or_else(|| captured.inherited.get(key))
            .cloned()
            .or_else(|| captured.globals.get(key))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed("Liquid::SelfDrop")
    }

    fn identity(&self) -> Option<String> {
        Some(format!("self:{}", self.scope))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Numbers the contexts, so that a `SelfDrop` knows the one it belongs to.
static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

fn next_scope() -> u64 {
    NEXT_SCOPE.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interrupt {
    Break,
    Continue,
}

/// State shared by a context and every isolated sub-context created from it.
pub struct Shared {
    pub env: Arc<Environment>,
    globals: Arc<dyn Object>,
    partials: Arc<dyn PartialLoader>,
    registers: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    errors: Mutex<Vec<Error>>,
    warnings: Mutex<Vec<String>>,
    /// The instant `'now'` resolves to. Fixing it makes renders reproducible.
    pub now: DateTime<Utc>,
    /// The time zone dates are displayed in.
    pub time_zone: Tz,
    /// Records what the render spends its time in, when it is asked to.
    profiler: Option<Arc<Profiler>>,
}

pub struct ContextBuilder {
    env: Arc<Environment>,
    globals: Arc<dyn Object>,
    partials: Arc<dyn PartialLoader>,
    registers: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    assigns: HashMap<String, Value>,
    now: Option<DateTime<Utc>>,
    time_zone: Tz,
    template_name: Option<Arc<str>>,
    profiler: Option<Arc<Profiler>>,
}

impl ContextBuilder {
    /// The object that resolves global variables (`shop`, `settings`, ...).
    pub fn globals(mut self, globals: Arc<dyn Object>) -> Self {
        self.globals = globals;
        self
    }

    pub fn partials(mut self, partials: Arc<dyn PartialLoader>) -> Self {
        self.partials = partials;
        self
    }

    /// Registers a value tags and filters can retrieve with [`Context::register`].
    pub fn register<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.registers.insert(TypeId::of::<T>(), value);
        self
    }

    /// Defines a variable visible to the template being rendered.
    pub fn assign(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.assigns.insert(name.into(), value.into());
        self
    }

    pub fn now(mut self, now: DateTime<Utc>) -> Self {
        self.now = Some(now);
        self
    }

    pub fn time_zone(mut self, time_zone: Tz) -> Self {
        self.time_zone = time_zone;
        self
    }

    pub fn template_name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.template_name = Some(name.into());
        self
    }

    /// Records what the render spends its time in.
    pub fn profiler(mut self, profiler: Arc<Profiler>) -> Self {
        self.profiler = Some(profiler);
        self
    }

    pub fn build(self) -> Context {
        Context {
            shared: Arc::new(Shared {
                env: self.env,
                globals: self.globals,
                partials: self.partials,
                registers: self.registers,
                errors: Mutex::new(Vec::new()),
                warnings: Mutex::new(Vec::new()),
                now: self.now.unwrap_or_else(Utc::now),
                time_zone: self.time_zone,
                profiler: self.profiler,
            }),
            scopes: vec![HashMap::new()],
            environment: self.assigns,
            inherited: Arc::new(HashMap::new()),
            interrupts: Vec::new(),
            cycles: HashMap::new(),
            for_offsets: HashMap::new(),
            for_stack: Vec::new(),
            ifchanged: None,
            template_name: self.template_name,
            base_depth: 0,
            include_disabled: false,
            scope: next_scope(),
        }
    }
}

pub struct Context {
    shared: Arc<Shared>,
    /// Outermost scope first. `assign` writes to the outermost one, loop variables to the innermost.
    scopes: Vec<HashMap<String, Value>>,
    /// Variables passed to the render call, plus `increment`/`decrement` counters.
    environment: HashMap<String, Value>,
    /// Variables that isolated sub-contexts inherit, unlike local ones. Shopify uses this for
    /// `section` and `block`, which snippets can read without receiving them as arguments.
    inherited: Arc<HashMap<String, Value>>,
    interrupts: Vec<Interrupt>,
    cycles: HashMap<String, usize>,
    for_offsets: HashMap<String, usize>,
    for_stack: Vec<Value>,
    ifchanged: Option<String>,
    pub template_name: Option<Arc<str>>,
    base_depth: usize,
    /// Set inside `render`, where `include` is not allowed.
    include_disabled: bool,
    /// Identifies this context to the `self` objects read in it.
    scope: u64,
}

impl Context {
    pub fn builder(env: Arc<Environment>) -> ContextBuilder {
        ContextBuilder {
            env,
            globals: Arc::new(NoGlobals),
            partials: Arc::new(NoPartials),
            registers: HashMap::new(),
            assigns: HashMap::new(),
            now: None,
            time_zone: Tz::UTC,
            template_name: None,
            profiler: None,
        }
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// What records this render, when it is profiled.
    pub fn profiler(&self) -> Option<&Arc<Profiler>> {
        self.shared.profiler.as_ref()
    }

    /// A value registered with [`ContextBuilder::register`].
    pub fn register<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.shared
            .registers
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }

    /// Creates the context a `render`ed partial runs in: it shares globals, registers and the
    /// error list but none of the variables.
    pub fn isolated(&self) -> Result<Context> {
        if self.base_depth + self.scopes.len() > MAX_DEPTH {
            return Err(Error::stack_level());
        }
        Ok(Context {
            shared: self.shared.clone(),
            scopes: vec![HashMap::new()],
            environment: HashMap::new(),
            inherited: self.inherited.clone(),
            interrupts: Vec::new(),
            cycles: HashMap::new(),
            for_offsets: HashMap::new(),
            for_stack: Vec::new(),
            ifchanged: None,
            template_name: self.template_name.clone(),
            base_depth: self.base_depth + 1,
            include_disabled: self.include_disabled,
            scope: next_scope(),
        })
    }

    // --- variables -------------------------------------------------------------------------

    /// Resolves a top-level variable: local scopes, then render assigns and counters, then
    /// globals. `self` is the scope itself unless a variable has that name.
    pub fn find_variable(&self, name: &str) -> Value {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return value.clone();
            }
        }
        if let Some(value) = self.environment.get(name)
            && !value.is_nil()
        {
            return value.clone();
        }
        if let Some(value) = self.inherited.get(name) {
            return value.clone();
        }
        match self.shared.globals.get(name) {
            Some(value) if !value.is_nil() => value,
            _ if name == "self" => Value::object(SelfDrop {
                scope: self.scope,
                captured: None,
            }),
            _ => Value::Nil,
        }
    }

    /// Whether `value` is the `self` of this context, whose lookups are variable lookups here.
    pub fn is_self(&self, value: &Value) -> bool {
        value
            .downcast::<SelfDrop>()
            .is_some_and(|drop| drop.scope == self.scope)
    }

    /// Prepares a value that leaves this context, for a partial or a filter: `self` takes the
    /// variables it stands for along.
    pub fn detach(&self, value: Value) -> Value {
        if !self.is_self(&value) {
            return value;
        }
        let mut variables: HashMap<String, Value> = self
            .environment
            .iter()
            .filter(|(_, value)| !value.is_nil())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        for scope in &self.scopes {
            variables.extend(
                scope
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        Value::object(SelfDrop {
            scope: self.scope,
            captured: Some(Captured {
                variables,
                inherited: self.inherited.clone(),
                globals: self.shared.globals.clone(),
            }),
        })
    }

    /// Sets a variable that this context and every sub-context created from it can read.
    pub fn set_inherited(&mut self, name: impl Into<String>, value: Value) {
        Arc::make_mut(&mut self.inherited).insert(name.into(), value);
    }

    /// Whether a local variable (not a global) with this name exists.
    pub fn has_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains_key(name))
    }

    /// Sets a variable in the innermost scope (loop variables, partial arguments).
    pub fn set(&mut self, name: impl Into<String>, value: Value) {
        self.scopes
            .last_mut()
            .expect("a context always has a scope")
            .insert(name.into(), value);
    }

    /// Sets a variable in the outermost scope, which is what `assign` and `capture` do.
    pub fn assign(&mut self, name: impl Into<String>, value: Value) {
        self.scopes[0].insert(name.into(), value);
    }

    pub fn push_scope(&mut self) -> Result<()> {
        self.scopes.push(HashMap::new());
        if self.base_depth + self.scopes.len() > MAX_DEPTH {
            self.scopes.pop();
            return Err(Error::stack_level());
        }
        Ok(())
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Runs `f` inside a new scope.
    pub fn with_scope<T>(&mut self, f: impl FnOnce(&mut Context) -> Result<T>) -> Result<T> {
        self.push_scope()?;
        let result = f(self);
        self.pop_scope();
        result
    }

    /// The counter storage used by `increment` and `decrement`.
    pub fn counter(&mut self, name: &str) -> &mut Value {
        self.environment
            .entry(name.to_string())
            .or_insert(Value::Int(0))
    }

    // --- control flow ----------------------------------------------------------------------

    pub fn push_interrupt(&mut self, interrupt: Interrupt) {
        self.interrupts.push(interrupt);
    }

    pub fn pop_interrupt(&mut self) -> Option<Interrupt> {
        self.interrupts.pop()
    }

    pub fn has_interrupt(&self) -> bool {
        !self.interrupts.is_empty()
    }

    pub fn cycle_index(&mut self, key: &str) -> &mut usize {
        self.cycles.entry(key.to_string()).or_insert(0)
    }

    pub fn for_offset(&self, name: &str) -> usize {
        self.for_offsets.get(name).copied().unwrap_or(0)
    }

    pub fn set_for_offset(&mut self, name: &str, offset: usize) {
        self.for_offsets.insert(name.to_string(), offset);
    }

    pub fn for_stack(&mut self) -> &mut Vec<Value> {
        &mut self.for_stack
    }

    pub fn ifchanged(&mut self) -> &mut Option<String> {
        &mut self.ifchanged
    }

    pub fn include_disabled(&self) -> bool {
        self.include_disabled
    }

    pub fn set_include_disabled(&mut self, disabled: bool) {
        self.include_disabled = disabled;
    }

    // --- filters, partials, clock ----------------------------------------------------------

    pub fn invoke_filter(&self, name: &str, input: &Value, args: &FilterArgs) -> Result<Value> {
        match self.shared.env.filter(name) {
            Some(filter) => filter(input, args, self),
            None => {
                self.warn(format!("unknown filter '{name}'"));
                Ok(input.clone())
            }
        }
    }

    pub fn load_partial(&self, name: &str) -> Result<Arc<Template>> {
        self.shared.partials.load(name)
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.shared.now
    }

    pub fn time_zone(&self) -> Tz {
        self.shared.time_zone
    }

    // --- diagnostics -----------------------------------------------------------------------

    /// Records an error and returns the text Liquid prints in its place.
    pub fn handle_error(&self, error: Error, line: u32) -> String {
        let error = if error.kind == ErrorKind::Internal {
            Error::internal()
        } else {
            error
        };
        let error = error
            .with_template(self.template_name.clone())
            .with_line(line);
        let message = error.to_string();
        if let Ok(mut errors) = self.shared.errors.lock() {
            errors.push(error);
        }
        message
    }

    /// Records a non-fatal diagnostic (unknown filter, missing translation, ...).
    pub fn warn(&self, message: impl Into<String>) {
        let message = match &self.template_name {
            Some(name) => format!("{name}: {}", message.into()),
            None => message.into(),
        };
        if let Ok(mut warnings) = self.shared.warnings.lock()
            && !warnings.contains(&message)
        {
            warnings.push(message);
        }
    }

    /// The errors rendered so far, across this context and its sub-contexts.
    pub fn errors(&self) -> Vec<Error> {
        self.shared
            .errors
            .lock()
            .map(|errors| errors.clone())
            .unwrap_or_default()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.shared
            .warnings
            .lock()
            .map(|warnings| warnings.clone())
            .unwrap_or_default()
    }
}
