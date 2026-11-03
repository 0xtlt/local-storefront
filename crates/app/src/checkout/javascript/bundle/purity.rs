//! Whether running a statement does something else than declare what it declares, as esbuild
//! judges it.

use oxc::ast::ast::{
    ArrayExpressionElement, Class, ClassElement, Declaration, Expression, ObjectPropertyKind,
    PropertyKey, UnaryOperator,
};

use super::*;

/// Whether running `expression` only computes a value, as esbuild judges it: no call but those
/// marked `/* @__PURE__ */` (as oxc marks the functions that build enums), no global that may
/// not exist, no spread.
pub(super) fn expression_is_pure(expression: &Expression, reader: &Reader) -> bool {
    let pure = |expression: &Expression| expression_is_pure(expression, reader);
    match expression {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.iter().all(pure),
        Expression::Identifier(reference) => {
            !is_global(reader.scoping, reference)
                || matches!(reference.name.as_str(), "undefined" | "NaN" | "Infinity")
        }
        Expression::ClassExpression(class) => class_is_pure(class, reader),
        Expression::ObjectExpression(object) => {
            object.properties.iter().all(|property| match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    key_is_pure(&property.key, reader) && pure(&property.value)
                }
                ObjectPropertyKind::SpreadProperty(_) => false,
            })
        }
        Expression::ArrayExpression(array) => array.elements.iter().all(|element| match element {
            ArrayExpressionElement::SpreadElement(_) => false,
            ArrayExpressionElement::Elision(_) => true,
            element => element.as_expression().is_some_and(pure),
        }),
        Expression::UnaryExpression(unary) => match unary.operator {
            UnaryOperator::Delete => false,
            // `typeof name` does not throw for a global that does not exist.
            UnaryOperator::Typeof if matches!(&unary.argument, Expression::Identifier(_)) => true,
            _ => pure(&unary.argument),
        },
        Expression::CallExpression(call) => {
            call.pure
                && call
                    .arguments
                    .iter()
                    .all(|argument| argument.as_expression().is_some_and(pure))
        }
        Expression::NewExpression(new) => {
            new.pure
                && new
                    .arguments
                    .iter()
                    .all(|argument| argument.as_expression().is_some_and(pure))
        }
        Expression::ParenthesizedExpression(inner) => pure(&inner.expression),
        Expression::SequenceExpression(sequence) => sequence.expressions.iter().all(pure),
        Expression::ConditionalExpression(conditional) => {
            pure(&conditional.test) && pure(&conditional.consequent) && pure(&conditional.alternate)
        }
        Expression::LogicalExpression(logical) => pure(&logical.left) && pure(&logical.right),
        _ => false,
    }
}

/// Whether a property key is computed without side effects: a name, a literal.
pub(super) fn key_is_pure(key: &PropertyKey, reader: &Reader) -> bool {
    match key {
        PropertyKey::StaticIdentifier(_) | PropertyKey::PrivateIdentifier(_) => true,
        key => key.as_expression().is_some_and(|key| {
            matches!(
                key,
                Expression::StringLiteral(_) | Expression::NumericLiteral(_)
            ) || !matches!(key, Expression::Identifier(_)) && expression_is_pure(key, reader)
        }),
    }
}

pub(super) fn class_is_pure(class: &Class, reader: &Reader) -> bool {
    let value_is_pure = |is_static: bool, value: Option<&Expression>| {
        !is_static || value.is_none_or(|value| expression_is_pure(value, reader))
    };
    class.decorators.is_empty()
        && class.heritage.as_ref().is_none_or(|heritage| {
            matches!(heritage.expression, Expression::Identifier(_))
                && expression_is_pure(&heritage.expression, reader)
        })
        && class.body.body.iter().all(|element| match element {
            ClassElement::StaticBlock(_) => false,
            ClassElement::MethodDefinition(method) => {
                method.decorators.is_empty() && key_is_pure(&method.key, reader)
            }
            ClassElement::PropertyDefinition(property) => {
                property.decorators.is_empty()
                    && key_is_pure(&property.key, reader)
                    && value_is_pure(property.r#static, property.value.as_ref())
            }
            ClassElement::AccessorProperty(accessor) => {
                accessor.decorators.is_empty()
                    && key_is_pure(&accessor.key, reader)
                    && value_is_pure(accessor.r#static, accessor.value.as_ref())
            }
            ClassElement::TSIndexSignature(_) => true,
        })
}

pub(super) fn declaration_is_pure(declaration: &Declaration, reader: &Reader) -> bool {
    match declaration {
        Declaration::VariableDeclaration(variables) => {
            variables.declarations.iter().all(|declarator| {
                declarator
                    .init
                    .as_ref()
                    .is_none_or(|init| expression_is_pure(init, reader))
            })
        }
        Declaration::ClassDeclaration(class) => class_is_pure(class, reader),
        _ => true,
    }
}
