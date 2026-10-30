//! The first reading of a module: its imports and exports, and for each top-level statement
//! what it declares and uses, and whether it has side effects.

use std::collections::{BTreeMap, HashMap};

use oxc::ast::ast::{
    Argument, CallExpression, Declaration, ExportDefaultDeclarationKind, Expression,
    IdentifierReference, ImportDeclarationSpecifier, ImportExpression, Program,
    SimpleAssignmentTarget, Statement, StaticMemberExpression,
};
use oxc::ast_visit::{Visit, walk};
use oxc::semantic::{ScopeFlags, Scoping};

use crate::javascript::resolve::Kind;

use super::*;

/// The names a declaration binds.
pub(super) fn declared(declaration: &Declaration) -> Vec<String> {
    match declaration {
        Declaration::VariableDeclaration(variables) => variables
            .declarations
            .iter()
            .flat_map(|declarator| declarator.id.get_binding_identifiers())
            .map(|binding| binding.name.to_string())
            .collect(),
        Declaration::FunctionDeclaration(function) => {
            function.id.iter().map(|id| id.name.to_string()).collect()
        }
        Declaration::ClassDeclaration(class) => {
            class.id.iter().map(|id| id.name.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// Whether `reference` names no binding: a global.
pub(super) fn is_global(scoping: &Scoping, reference: &IdentifierReference) -> bool {
    reference
        .reference_id
        .get()
        .is_none_or(|id| scoping.get_reference(id).symbol_id().is_none())
}

/// The literal of a `require("…")` call, if that is what `call` is: `Err` when it requires
/// something else than a string.
pub(super) fn required(call: &CallExpression, scoping: &Scoping) -> Option<Result<String, ()>> {
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    if callee.name != "require" || !is_global(scoping, callee) {
        return None;
    }
    Some(match (call.arguments.len(), call.arguments.first()) {
        (1, Some(Argument::StringLiteral(literal))) => Ok(literal.value.to_string()),
        _ => Err(()),
    })
}

/// Reads a module (first phase): its imports, exports, and for each top-level statement what it
/// declares, uses and whether it has side effects.
pub(super) struct Reader<'s, 'r> {
    pub(super) text: &'s str,
    pub(super) name: &'s str,
    /// Whether the module was TypeScript.
    pub(super) typescript: bool,
    pub(super) scoping: &'s Scoping,
    pub(super) resolve: &'r mut dyn FnMut(&str, Kind) -> Result<usize, String>,
    pub(super) module: usize,
    pub(super) requests: Vec<(String, usize)>,
    pub(super) imports: HashMap<String, Import>,
    pub(super) declared_by: HashMap<String, Vec<usize>>,
    pub(super) exports: BTreeMap<String, Export>,
    pub(super) stars: Vec<usize>,
    pub(super) named: Vec<NamedImport>,
    /// The statement being read.
    pub(super) statement: StatementUse,
    pub(super) error: Option<String>,
}

impl Reader<'_, '_> {
    pub(super) fn line(&self, offset: u32) -> usize {
        position(self.text, offset as usize).0
    }

    /// The module `specifier` names, as an ES module import.
    pub(super) fn request(&mut self, specifier: &str) -> Result<usize, String> {
        if let Some((_, id)) = self.requests.iter().find(|(name, _)| name == specifier) {
            return Ok(*id);
        }
        let id = (self.resolve)(specifier, Kind::Import)?;
        self.requests.push((specifier.to_string(), id));
        Ok(id)
    }

    pub(super) fn named(&mut self, from: usize, name: &str, offset: u32) {
        let line = self.line(offset);
        self.named.push(NamedImport {
            module: self.module,
            from,
            name: name.to_string(),
            line,
            maybe_type: false,
        });
    }

    pub(super) fn read(&mut self, program: &Program) -> Result<Vec<StatementUse>, String> {
        // The imports first: they are hoisted, and the modules they name run in the order of the
        // statements that name them.
        for statement in &program.body {
            match statement {
                Statement::ImportDeclaration(import) => {
                    let from = self.request(&import.source.value)?;
                    for specifier in import.specifiers.iter().flatten() {
                        let (local, name) = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(named) => {
                                let imported = named.imported.name().to_string();
                                self.named(from, &imported, named.span.start);
                                (&named.local, Some(imported))
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => {
                                self.named(from, "default", default.span.start);
                                (&default.local, Some("default".to_string()))
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(namespace) => {
                                (&namespace.local, None)
                            }
                        };
                        self.imports
                            .insert(local.name.to_string(), Import { from, name });
                    }
                }
                Statement::ExportFromDeclaration(export) => {
                    self.request(&export.source.value)?;
                }
                Statement::ExportAllDeclaration(export) => {
                    self.request(&export.source.value)?;
                }
                _ => {}
            }
        }

        let mut statements = Vec::with_capacity(program.body.len());
        for (index, statement) in program.body.iter().enumerate() {
            self.statement = StatementUse::default();
            match statement {
                Statement::ImportDeclaration(_) => {}
                Statement::ExportFromDeclaration(export) => {
                    let from = self.request(&export.source.value)?;
                    for specifier in &export.specifiers {
                        let local = specifier.local.name().to_string();
                        self.named(from, &local, specifier.span.start);
                        if self.typescript {
                            self.named.last_mut().expect("just pushed").maybe_type = true;
                        }
                        self.exports.insert(
                            specifier.exported.name().to_string(),
                            Export::From(from, local),
                        );
                    }
                }
                Statement::ExportAllDeclaration(export) => {
                    let from = self.request(&export.source.value)?;
                    match &export.exported {
                        Some(name) => {
                            self.exports
                                .insert(name.name().to_string(), Export::Namespace(from));
                        }
                        None => self.stars.push(from),
                    }
                }
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        let exported = specifier.exported.name().to_string();
                        let local = specifier.local.name().to_string();
                        let target = match self.imports.get(&local) {
                            Some(Import {
                                from,
                                name: Some(name),
                            }) => Export::From(*from, name.clone()),
                            Some(Import { from, name: None }) => Export::Namespace(*from),
                            None => Export::Local(local),
                        };
                        self.exports.insert(exported, target);
                    }
                }
                Statement::ExportDeclaration(export) => {
                    for name in declared(&export.declaration) {
                        self.declare(&name, index);
                        self.exports.insert(name.clone(), Export::Local(name));
                    }
                    self.statement.effects = !declaration_is_pure(&export.declaration, self);
                    self.visit_declaration(&export.declaration);
                }
                Statement::ExportDefaultDeclaration(export) => {
                    let local = match &export.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                            self.visit_function(function, ScopeFlags::Function);
                            function.id.as_ref().map(|id| id.name.to_string())
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                            self.statement.effects = !class_is_pure(class, self);
                            self.visit_class(class);
                            class.id.as_ref().map(|id| id.name.to_string())
                        }
                        kind => {
                            if let Some(expression) = kind.as_expression() {
                                self.statement.effects = !expression_is_pure(expression, self);
                                self.visit_expression(expression);
                            }
                            None
                        }
                    };
                    let local = local.unwrap_or_else(|| "__lsf_default".to_string());
                    self.declare(&local, index);
                    self.exports
                        .insert("default".to_string(), Export::Local(local));
                }
                statement => {
                    match statement {
                        Statement::FunctionDeclaration(function) => {
                            if let Some(id) = &function.id {
                                self.declare(&id.name, index);
                            }
                        }
                        Statement::ClassDeclaration(class) => {
                            if let Some(id) = &class.id {
                                self.declare(&id.name, index);
                            }
                            self.statement.effects = !class_is_pure(class, self);
                        }
                        Statement::VariableDeclaration(variables) => {
                            for declarator in &variables.declarations {
                                for binding in declarator.id.get_binding_identifiers() {
                                    self.declare(&binding.name, index);
                                }
                            }
                            self.statement.effects = !variables.declarations.iter().all(|d| {
                                d.init
                                    .as_ref()
                                    .is_none_or(|init| expression_is_pure(init, self))
                            });
                        }
                        Statement::EmptyStatement(_) => {}
                        _ => self.statement.effects = true,
                    }
                    self.visit_statement(statement);
                }
            }
            if let Some(error) = self.error.take() {
                return Err(error);
            }
            statements.push(std::mem::take(&mut self.statement));
        }
        Ok(statements)
    }

    pub(super) fn declare(&mut self, name: &str, statement: usize) {
        self.declared_by
            .entry(name.to_string())
            .or_default()
            .push(statement);
    }

    /// The top-level name a reference reads, if it reads one (not a local of a function, not a
    /// global).
    pub(super) fn top_level(&self, reference: &IdentifierReference) -> Option<String> {
        let symbol = self
            .scoping
            .get_reference(reference.reference_id.get()?)
            .symbol_id()?;
        (self.scoping.symbol_scope_id(symbol) == self.scoping.root_scope_id())
            .then(|| self.scoping.symbol_name(symbol).to_string())
    }

    pub(super) fn fail(&mut self, offset: u32, message: String) {
        if self.error.is_none() {
            let line = self.line(offset);
            self.error = Some(format!("{}:{line}: {message}", self.name));
        }
    }
}

impl<'a> Visit<'a> for Reader<'_, '_> {
    fn visit_identifier_reference(&mut self, reference: &IdentifierReference<'a>) {
        if let Some(name) = self.top_level(reference) {
            self.statement.uses.push(Use::Name(name));
        }
    }

    fn visit_static_member_expression(&mut self, expression: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(object) = &expression.object
            && let Some(name) = self.top_level(object)
            && self.imports.contains_key(&name)
        {
            self.statement
                .uses
                .push(Use::Member(name, expression.property.name.to_string()));
            return;
        }
        walk::walk_static_member_expression(self, expression);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        // Writing `name.property` uses `name` itself.
        if let SimpleAssignmentTarget::StaticMemberExpression(expression) = target
            && let Expression::Identifier(object) = &expression.object
            && let Some(name) = self.top_level(object)
        {
            self.statement.uses.push(Use::Name(name));
            return;
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        match required(call, self.scoping) {
            Some(Ok(specifier)) => match (self.resolve)(&specifier, Kind::Require) {
                Ok(id) => self.statement.requires.push((call.span.start, id)),
                Err(error) => {
                    if self.error.is_none() {
                        self.error = Some(error);
                    }
                }
            },
            Some(Err(())) => self.fail(
                call.span.start,
                "`require` of something else than a string cannot be bundled".to_string(),
            ),
            None => walk::walk_call_expression(self, call),
        }
    }

    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        self.fail(
            import.span.start,
            "`import()` cannot be bundled into a Function: import the module".to_string(),
        );
    }
}
