//! Where each module goes in a bundle, the names it uses, and its text: ES modules in one scope,
//! CommonJS modules as functions.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    AssignmentTargetPropertyIdentifier, BindingIdentifier, BindingProperty, CallExpression,
    ExportDefaultDeclarationKind, Expression, IdentifierReference, ObjectProperty,
    SimpleAssignmentTarget, Statement, StaticMemberExpression, TaggedTemplateExpression,
};
use oxc::ast_visit::{Visit, walk};
use oxc::parser::Parser;
use oxc::semantic::{ScopeFlags, Scoping, SemanticBuilder, SymbolId};
use oxc::span::{GetSpan, SourceType, Span};

use super::*;

/// Where the modules go and the names they use, in the bundle.
#[derive(Default)]
pub(super) struct Layout {
    /// The ES modules, in the order they run.
    pub(super) order: Vec<usize>,
    /// The CommonJS modules and JSON files.
    pub(super) commonjs: Vec<usize>,
    /// The final name of each top-level name of an ES module.
    pub(super) names: HashMap<(usize, String), String>,
    /// The object of each ES module read as a namespace.
    pub(super) namespaces: HashMap<usize, String>,
    /// The function that runs each CommonJS module.
    pub(super) wrappers: HashMap<usize, String>,
    /// The view of each CommonJS module as an ES module.
    pub(super) interop: HashMap<usize, String>,
    /// The module that makes that view, the first to import the CommonJS module.
    pub(super) owner: HashMap<usize, usize>,
}

impl Layout {
    /// The expression that reads `target`, and whether it is a plain name.
    pub(super) fn expression(&self, target: Target) -> (String, bool) {
        self.read(&target)
            .expect("what the Function needs has a name in the bundle")
    }

    /// The expression that reads `target`, and whether it is a plain name; `None` for a name
    /// no module declares (a type re-exported in TypeScript), or a namespace that has no object.
    pub(super) fn read(&self, target: &Target) -> Option<(String, bool)> {
        Some(match target {
            Target::Local(id, name) => (self.names.get(&(*id, name.clone()))?.clone(), true),
            Target::Namespace(id) => (self.namespaces.get(id)?.clone(), true),
            Target::CommonJs(id, None) => (self.interop.get(id)?.clone(), true),
            Target::CommonJs(id, Some(name)) => (member(self.interop.get(id)?, name), false),
        })
    }
}

/// Writes a module (second phase): its imports read where they are declared, its top-level names
/// renamed, enum members inlined, `require`s run.
pub(super) struct Writer<'s, 'g> {
    pub(super) graph: &'s Graph<'g>,
    pub(super) layout: &'s Layout,
    pub(super) module: usize,
    pub(super) scoping: &'s Scoping,
    pub(super) edits: Vec<(u32, u32, String)>,
    /// What each imported binding reads, and whether that is a plain name.
    pub(super) reads: HashMap<SymbolId, (String, bool)>,
    /// The namespace imports (`* as name`), and the module of each.
    pub(super) namespaces: HashMap<SymbolId, usize>,
    /// The top-level bindings renamed.
    pub(super) renames: HashMap<SymbolId, String>,
    /// Imported bindings called (`f()`, f`…`): called as `(0, object.f)()` when what they read
    /// is a property, so that the call has no `this`.
    pub(super) callees: BTreeSet<u32>,
    /// Names in shorthand properties and patterns (`{ f }`), written `{ f: renamed }`.
    pub(super) shorthands: BTreeSet<u32>,
    /// Member expressions written to: never inlined.
    pub(super) targets: BTreeSet<u32>,
    /// The modules of the module's `require` calls, by where the call starts.
    pub(super) requires: &'s BTreeMap<u32, usize>,
}

impl<'s, 'g> Writer<'s, 'g> {
    pub(super) fn new(
        graph: &'s Graph<'g>,
        module: usize,
        scoping: &'s Scoping,
        layout: &'s Layout,
    ) -> Self {
        Writer {
            graph,
            layout,
            module,
            scoping,
            edits: Vec::new(),
            reads: HashMap::new(),
            namespaces: HashMap::new(),
            renames: HashMap::new(),
            callees: BTreeSet::new(),
            shorthands: BTreeSet::new(),
            targets: BTreeSet::new(),
            requires: &graph.modules[module].requires,
        }
    }

    pub(super) fn edit(&mut self, span: Span, replacement: impl Into<String>) {
        self.edits.push((span.start, span.end, replacement.into()));
    }

    pub(super) fn symbol(&self, reference: &IdentifierReference) -> Option<SymbolId> {
        self.scoping
            .get_reference(reference.reference_id.get()?)
            .symbol_id()
    }
}

impl<'a> Visit<'a> for Writer<'_, '_> {
    fn visit_identifier_reference(&mut self, reference: &IdentifierReference<'a>) {
        let Some(symbol) = self.symbol(reference) else {
            return;
        };
        let span = reference.span;
        let (read, plain) = match (self.reads.get(&symbol), self.renames.get(&symbol)) {
            (Some(read), _) => read.clone(),
            (None, Some(renamed)) => (renamed.clone(), true),
            (None, None) => return,
        };
        let replacement = if self.shorthands.contains(&span.start) {
            format!("{}: {read}", reference.name)
        } else if !plain && self.callees.contains(&span.start) {
            format!("(0, {read})")
        } else {
            read
        };
        self.edit(span, replacement);
    }

    fn visit_binding_identifier(&mut self, binding: &BindingIdentifier<'a>) {
        let Some(renamed) = binding
            .symbol_id
            .get()
            .and_then(|symbol| self.renames.get(&symbol))
            .cloned()
        else {
            return;
        };
        let replacement = if self.shorthands.contains(&binding.span.start) {
            format!("{}: {renamed}", binding.name)
        } else {
            renamed
        };
        self.edit(binding.span, replacement);
    }

    fn visit_binding_property(&mut self, property: &BindingProperty<'a>) {
        if property.shorthand {
            self.shorthands.insert(property.key.span().start);
        }
        walk::walk_binding_property(self, property);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        property: &AssignmentTargetPropertyIdentifier<'a>,
    ) {
        self.shorthands.insert(property.binding.span.start);
        walk::walk_assignment_target_property_identifier(self, property);
    }

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        if property.shorthand
            && let Expression::Identifier(value) = &property.value
        {
            self.shorthands.insert(value.span.start);
        }
        walk::walk_object_property(self, property);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if required(call, self.scoping).is_some() {
            let id = self.requires[&call.span.start];
            let call_text = format!("{}()", self.layout.wrappers[&id]);
            self.edit(call.span, call_text);
            return;
        }
        if let Expression::Identifier(callee) = &call.callee {
            self.callees.insert(callee.span.start);
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_tagged_template_expression(&mut self, tagged: &TaggedTemplateExpression<'a>) {
        if let Expression::Identifier(tag) = &tagged.tag {
            self.callees.insert(tag.span.start);
        }
        walk::walk_tagged_template_expression(self, tagged);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        if let SimpleAssignmentTarget::StaticMemberExpression(expression) = target {
            self.targets.insert(expression.span.start);
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_static_member_expression(&mut self, expression: &StaticMemberExpression<'a>) {
        let property = &expression.property.name;
        if !self.targets.contains(&expression.span.start)
            && let Expression::Identifier(object) = &expression.object
            && let Some(symbol) = self.symbol(object)
        {
            if let Some(constant) =
                self.graph
                    .enum_member(self.module, self.scoping.symbol_name(symbol), property)
            {
                // `-Dir.Down` must not become `--1`.
                let replacement = if constant.starts_with('-') {
                    format!("({constant} /* {property} */)")
                } else {
                    format!("{constant} /* {property} */")
                };
                self.edit(expression.span, replacement);
                return;
            }
            // `namespace.name` reads `name` where it is declared, as esbuild does.
            if let Some(&from) = self.namespaces.get(&symbol)
                && let Some((read, _)) = self.layout.read(&self.graph.target(from, Some(property)))
            {
                self.edit(expression.span, read);
                return;
            }
        }
        if let Expression::StaticMemberExpression(object) = &expression.object
            && object.property.name == "env"
            && let Expression::Identifier(process) = &object.object
            && process.name == "process"
            && is_global(self.scoping, process)
            && let Some(value) = self.graph.env.get(expression.property.name.as_str())
        {
            let literal = quote(value);
            self.edit(expression.span, literal);
            return;
        }
        walk::walk_static_member_expression(self, expression);
    }
}

impl Graph<'_> {
    /// Where each module goes and the names it uses: the ES modules in the order they run
    /// (each after what it imports), the CommonJS ones as functions first.
    pub(super) fn layout(&self, live: &Live, entries: &[usize]) -> Layout {
        let mut layout = Layout::default();
        let mut seen = vec![false; self.modules.len()];
        for &entry in entries {
            self.visit(entry, live, &mut seen, &mut layout.order);
        }
        for (id, module) in self.modules.iter().enumerate() {
            if live.needed[id] && module.form != Form::Esm {
                layout.commonjs.push(id);
                layout.wrappers.insert(id, format!("__lsf_commonjs{id}"));
                layout.interop.insert(id, format!("__lsf_import{id}"));
            }
        }
        for &id in &layout.order {
            if live.whole[id] {
                layout.namespaces.insert(id, format!("__lsf_namespace{id}"));
            }
            for &(_, request) in &self.modules[id].requests {
                if layout.wrappers.contains_key(&request) {
                    layout.owner.entry(request).or_insert(id);
                }
            }
        }

        // A top-level name keeps its name unless another module declares the same (in any
        // scope: an import of it there would read that one), a module reads a global of that
        // name, or a module before took it.
        let mut declaring: HashMap<&str, usize> = HashMap::new();
        // The globals the CommonJS modules and the runtime read, as well as the ES modules: a
        // top-level name would hide them.
        let mut taken: BTreeSet<String> = BTreeSet::from(["Object".to_string()]);
        for &id in &layout.commonjs {
            taken.extend(self.modules[id].globals.iter().cloned());
        }
        for &id in &layout.order {
            let module = &self.modules[id];
            for symbol in &module.symbols {
                *declaring.entry(symbol).or_default() += 1;
            }
            taken.extend(module.globals.iter().cloned());
        }
        let all: BTreeSet<&str> = declaring.keys().copied().collect();
        for &id in &layout.order {
            let module = &self.modules[id];
            let mut names: Vec<(&usize, &String)> = module
                .declared_by
                .iter()
                .map(|(name, statements)| (statements.iter().min().expect("one"), name))
                .collect();
            names.sort();
            for (_, name) in names {
                let others = declaring.get(name.as_str()).copied().unwrap_or(0)
                    > usize::from(module.symbols.contains(name));
                let renamed = if taken.contains(name) || others {
                    (1..)
                        .map(|n| format!("{name}_{n}"))
                        .find(|candidate| {
                            !taken.contains(candidate) && !all.contains(candidate.as_str())
                        })
                        .expect("a free name")
                } else {
                    name.clone()
                };
                taken.insert(renamed.clone());
                layout.names.insert((id, name.clone()), renamed);
            }
        }
        layout
    }

    /// Puts the ES modules `id` imports, then `id`, in `order`.
    pub(super) fn visit(&self, id: usize, live: &Live, seen: &mut [bool], order: &mut Vec<usize>) {
        if seen[id] || !live.needed[id] || self.modules[id].form != Form::Esm {
            return;
        }
        seen[id] = true;
        for &(_, request) in &self.modules[id].requests {
            self.visit(request, live, seen, order);
        }
        order.push(id);
    }

    /// A CommonJS module, or a JSON file: a function that runs it the first time it is needed.
    pub(super) fn emit_commonjs(&self, id: usize, layout: &Layout) -> String {
        let module = &self.modules[id];
        let body = if module.form == Form::Json {
            format!("module.exports = {};", module.javascript.trim())
        } else {
            let allocator = Allocator::default();
            let source_type = SourceType::from_path("module.js")
                .expect("a known extension")
                .with_commonjs(true);
            let program = Parser::new(&allocator, &module.javascript, source_type)
                .parse()
                .program;
            let semantic = SemanticBuilder::new().build(&program);
            let mut writer = Writer::new(self, id, semantic.semantic.scoping(), layout);
            if let Some(hashbang) = &program.hashbang {
                writer.edit(hashbang.span, "");
            }
            for statement in &program.body {
                writer.visit_statement(statement);
            }
            apply(&module.javascript, &mut writer.edits)
        };
        format!(
            "// {}\nvar {} = __lsf_commonjs(function (exports, module) {{\n{body}\n}});\n",
            module.name, layout.wrappers[&id]
        )
    }

    /// An ES module, in the shared scope: what it does not need left out, its imports read
    /// where they are declared, its top-level names renamed where they clash.
    pub(super) fn emit(&self, id: usize, live: &Live, layout: &Layout) -> String {
        let module = &self.modules[id];
        let text = &module.javascript;
        let allocator = Allocator::default();
        let program = Parser::new(&allocator, text, javascript_type(SourceType::mjs()))
            .parse()
            .program;
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.scoping();
        let mut writer = Writer::new(self, id, scoping, layout);
        if let Some(hashbang) = &program.hashbang {
            writer.edit(hashbang.span, "");
        }
        for symbol in scoping.symbol_ids() {
            if scoping.symbol_scope_id(symbol) != scoping.root_scope_id() {
                continue;
            }
            let name = scoping.symbol_name(symbol);
            if let Some(import) = module.imports.get(name) {
                if import.name.is_none() {
                    writer.namespaces.insert(symbol, import.from);
                }
                // A namespace only read through its members has no object: they are read where
                // they are declared.
                if let Some(read) = layout.read(&self.target(import.from, import.name.as_deref())) {
                    writer.reads.insert(symbol, read);
                }
            } else if let Some(renamed) = layout.names.get(&(id, name.to_string()))
                && renamed != name
            {
                writer.renames.insert(symbol, renamed.clone());
            }
        }
        let default = || layout.names[&(id, "__lsf_default".to_string())].clone();

        for (index, statement) in program.body.iter().enumerate() {
            let kept = live.statements[id][index];
            match statement {
                Statement::ImportDeclaration(_)
                | Statement::ExportFromDeclaration(_)
                | Statement::ExportAllDeclaration(_)
                | Statement::ExportNamedDeclaration(_) => writer.edit(statement.span(), ""),
                _ if !kept => writer.edit(statement.span(), ""),
                Statement::ExportDeclaration(export) => {
                    let declaration = &export.declaration;
                    writer.edit(Span::new(export.span.start, declaration.span().start), "");
                    writer.visit_declaration(declaration);
                }
                Statement::ExportDefaultDeclaration(export) => {
                    let start = export.span.start;
                    match &export.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                            if function.id.is_some() {
                                writer.edit(Span::new(start, function.span.start), "");
                            } else {
                                let head = format!(
                                    "{}function{} {}",
                                    if function.r#async { "async " } else { "" },
                                    if function.generator { "*" } else { "" },
                                    default(),
                                );
                                writer.edit(Span::new(start, function.params.span.start), head);
                            }
                            writer.visit_function(function, ScopeFlags::Function);
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                            if class.id.is_some() {
                                writer.edit(Span::new(start, class.span.start), "");
                            } else {
                                writer.edit(
                                    Span::new(start, class.span.start + "class".len() as u32),
                                    format!("class {}", default()),
                                );
                            }
                            writer.visit_class(class);
                        }
                        kind => {
                            if let Some(expression) = kind.as_expression() {
                                writer.edit(
                                    Span::new(start, expression.span().start),
                                    format!("var {} = ", default()),
                                );
                                writer.visit_expression(expression);
                            } else {
                                writer.edit(export.span, "");
                            }
                        }
                    }
                }
                statement => writer.visit_statement(statement),
            }
        }
        let body = apply(text, &mut writer.edits);

        let mut head = format!("// {}\n", module.name);
        for &(_, request) in &module.requests {
            if layout.owner.get(&request) == Some(&id) {
                head.push_str(&format!(
                    "var {} = __lsf_toESM({}());\n",
                    layout.interop[&request], layout.wrappers[&request]
                ));
            }
        }
        if let Some(namespace) = layout.namespaces.get(&id) {
            let getters: Vec<String> = module
                .exports
                .keys()
                .filter_map(|name| {
                    let (value, _) = layout.read(&self.target(id, Some(name)))?;
                    Some(format!("{}: () => {value}", quote(name)))
                })
                .collect();
            head.push_str(&format!(
                "var {namespace} = {{}};\n__lsf_export({namespace}, {{ {} }});\n",
                getters.join(", ")
            ));
            for star in &module.stars {
                if let Some((source, _)) = layout.read(&self.target(*star, None)) {
                    head.push_str(&format!("__lsf_reexport({namespace}, {source});\n"));
                }
            }
        }
        format!("{head}{}\n", body.trim_end())
    }
}
