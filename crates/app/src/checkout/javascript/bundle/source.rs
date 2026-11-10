//! A module's JavaScript: TypeScript and JSX removed, the `jsxImportSource` its `tsconfig.json`
//! names, and the constant members of its enums.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, Expression, Program, Statement, TSEnumDeclaration, TSEnumMemberName, UnaryOperator,
};
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{JsxRuntime, TransformOptions, Transformer};

use super::*;

/// The JavaScript a module is read as, after its types are removed.
pub(super) fn javascript_type(source_type: SourceType) -> SourceType {
    SourceType::from_path("module.js")
        .expect("a known extension")
        .with_commonjs(source_type.is_commonjs())
}

/// The JavaScript of a TypeScript or JSX module: types, `import type`s and imports only used as
/// types removed, `enum`s and `namespace`s turned into objects, JSX turned into calls of the
/// automatic runtime of `jsx_source` (`preact/jsx-runtime`), nothing lowered past ES2022. With the
/// constant members of its top-level enums.
pub(super) fn transform(
    text: &str,
    source_type: SourceType,
    path: &Path,
    name: &str,
    jsx_source: &str,
) -> Result<(String, Enums), String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if let Some(error) = parsed.diagnostics.iter().next() {
        return Err(describe(error, name, text));
    }
    let mut program = parsed.program;
    let enums = constant_enums(&program);
    let semantic = SemanticBuilder::new().with_enum_eval(true).build(&program);
    let mut options = TransformOptions::from_target("es2022").expect("a known target");
    options.jsx.runtime = JsxRuntime::Automatic;
    options.jsx.import_source = Some(jsx_source.to_string());
    let transformed = Transformer::new(&allocator, path, &options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if let Some(error) = transformed.diagnostics.iter().next() {
        return Err(describe(error, name, text));
    }
    Ok((Codegen::new().build(&program).code, enums))
}

/// The `compilerOptions.jsxImportSource` of a `tsconfig.json`, or of the one it `extends`
/// (a path). The file may hold comments and trailing commas, as TypeScript allows.
pub(super) fn jsx_import_source(config: &Path, depth: usize) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    let value: serde_json::Value = serde_json::from_str(&jsonc(&text)).ok()?;
    if let Some(source) = value["compilerOptions"]["jsxImportSource"].as_str() {
        return Some(source.to_string());
    }
    let extends = value["extends"].as_str()?;
    if depth > 8 || !(extends.starts_with('.') || Path::new(extends).is_absolute()) {
        return None;
    }
    // As TypeScript: the path, else the path with `.json` after it.
    let parent = config.parent()?.join(extends);
    let parent = if parent.is_file() {
        parent
    } else {
        let mut with_json = parent.into_os_string();
        with_json.push(".json");
        PathBuf::from(with_json)
    };
    jsx_import_source(&parent, depth + 1)
}

/// JSON with comments and trailing commas, as plain JSON.
pub(super) fn jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(c);
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
            }
            (',', _) => {
                // A comma before `}` or `]` is dropped.
                let rest: String = chars.clone().collect();
                if !matches!(rest.trim_start().chars().next(), Some('}' | ']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The enums declared once at the top level whose members are all constants (string or number
/// literals, or numbers counted from the member before), each member written as JavaScript.
pub(super) fn constant_enums(program: &Program) -> Enums {
    let mut enums = Enums::new();
    let mut declared: HashMap<String, usize> = HashMap::new();
    for statement in &program.body {
        let declaration: &TSEnumDeclaration = match statement {
            Statement::TSEnumDeclaration(declaration) => declaration,
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::TSEnumDeclaration(declaration) => declaration,
                _ => continue,
            },
            _ => continue,
        };
        let name = declaration.id.name.to_string();
        *declared.entry(name.clone()).or_default() += 1;
        if declaration.declare {
            continue;
        }
        if let Some(members) = constant_members(declaration) {
            enums.insert(name, members);
        }
    }
    enums.retain(|name, _| declared[name] == 1);
    enums
}

pub(super) fn constant_members(declaration: &TSEnumDeclaration) -> Option<HashMap<String, String>> {
    let mut members = HashMap::new();
    let mut next: Option<f64> = Some(0.0);
    for member in &declaration.body.members {
        let key = match &member.id {
            TSEnumMemberName::Identifier(name) => name.name.to_string(),
            TSEnumMemberName::String(literal) | TSEnumMemberName::ComputedString(literal) => {
                literal.value.to_string()
            }
            TSEnumMemberName::ComputedTemplateString(_) => return None,
        };
        let value = match &member.initializer {
            None => {
                let number = next?;
                next = Some(number + 1.0);
                number_literal(number)
            }
            Some(Expression::StringLiteral(literal)) => {
                next = None;
                quote(&literal.value)
            }
            Some(Expression::NumericLiteral(literal)) => {
                next = Some(literal.value + 1.0);
                number_literal(literal.value)
            }
            Some(Expression::UnaryExpression(unary))
                if unary.operator == UnaryOperator::UnaryNegation =>
            {
                let Expression::NumericLiteral(literal) = &unary.argument else {
                    return None;
                };
                next = Some(-literal.value + 1.0);
                number_literal(-literal.value)
            }
            _ => return None,
        };
        members.insert(key, value);
    }
    Some(members)
}

/// A number as JavaScript writes it, for the integers enums count with and other literals.
pub(super) fn number_literal(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}
