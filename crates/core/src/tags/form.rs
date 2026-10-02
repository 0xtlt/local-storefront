//! `{% form 'type', object, attribute: value %}`.

use std::any::Any;

use lsf_liquid::filters::escape_html;
use lsf_liquid::lexer::{MarkupParser, TokenKind};
use lsf_liquid::{
    BlockBody, Context, Error, Expr, Hash, Object, Parser, Result, Tag, TagToken, Value,
};

use crate::drops::content::ArticleDrop;
use crate::drops::product::ProductDrop;
use crate::drops::shop::AddressDrop;
use crate::render::state::RenderState;
use crate::site::FormResult;

struct Form {
    form_type: Expr,
    object: Option<Expr>,
    attributes: Vec<(String, Expr)>,
    body: BlockBody,
}

/// The `form` object available inside a `form` block.
pub struct FormDrop {
    pub form_type: String,
    pub id: Option<String>,
    pub result: Option<FormResult>,
    /// Extra fields, e.g. the address being edited.
    pub fields: Hash,
}

impl Object for FormDrop {
    fn type_name(&self) -> &str {
        "form"
    }

    fn get(&self, key: &str) -> Option<Value> {
        match key {
            "id" => Some(self.id.as_ref().map_or(Value::Nil, Value::from)),
            "posted_successfully?" => Some(Value::Bool(
                self.result
                    .as_ref()
                    .is_some_and(|result| result.posted_successfully),
            )),
            "errors" => Some(match &self.result {
                Some(result) if !result.errors.is_empty() => {
                    Value::object(FormErrors(result.errors.clone()))
                }
                _ => Value::Nil,
            }),
            "password_needed" => Some(Value::Bool(true)),
            "set_as_default_checkbox" => Some(Value::str(
                "<input type=\"checkbox\" id=\"address_default_address_new\" name=\"address[default]\" value=\"1\">",
            )),
            field => self
                .result
                .as_ref()
                .and_then(|result| result.values.get(field))
                .map(Value::from)
                .or_else(|| self.fields.get(field).cloned()),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `form.errors`: iterates over the fields in error, with `messages` and `translated_fields`.
pub struct FormErrors(pub Vec<(String, String)>);

impl Object for FormErrors {
    fn type_name(&self) -> &str {
        "form_errors"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let by_field = |pick: &dyn Fn(&(String, String)) -> Value| {
            Value::hash(
                self.0
                    .iter()
                    .map(|error| (error.0.clone(), pick(error)))
                    .collect(),
            )
        };
        Some(match key {
            "messages" => by_field(&|(_, message)| Value::from(message)),
            "translated_fields" => {
                by_field(&|(field, _)| Value::from(crate::util::humanize(field)))
            }
            _ => return None,
        })
    }

    fn items(&self) -> Option<std::sync::Arc<Vec<Value>>> {
        Some(std::sync::Arc::new(
            self.0.iter().map(|(field, _)| Value::from(field)).collect(),
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The shape of the `<form>` element of each form type.
struct Shape {
    action: String,
    /// The action ends with `#<id of the form>`, which brings the browser back to the form
    /// once it is submitted.
    anchored: bool,
    id: Option<String>,
    class: Option<&'static str>,
    multipart: bool,
    /// Attributes printed after the standard ones.
    extra_attributes: &'static str,
    /// Hidden inputs after `utf8`.
    leading_inputs: Vec<(&'static str, String)>,
    /// Hidden inputs before `</form>`.
    trailing_inputs: Vec<(&'static str, String)>,
}

impl Shape {
    fn new(action: String) -> Self {
        Shape {
            action,
            anchored: false,
            id: None,
            class: None,
            multipart: false,
            extra_attributes: "",
            leading_inputs: Vec::new(),
            trailing_inputs: Vec::new(),
        }
    }
}

fn shape(
    ctx: &Context,
    state: &RenderState,
    form_type: &str,
    object: &Value,
) -> Result<(Shape, Hash)> {
    let site = &state.site;
    let path = |path: &str| site.request.localized(path);
    let current = site.request.path_with_query();
    let mut fields = Hash::new();
    let shape = match form_type {
        "product" => {
            let Some(product) = object.downcast::<ProductDrop>() else {
                return Err(Error::argument("product form must be given a product"));
            };
            let id = product.product().id;
            let mut shape = Shape::new(path("/cart/add"));
            shape.id = Some(format!("product_form_{id}"));
            shape.class = Some("shopify-product-form");
            shape.multipart = true;
            shape.trailing_inputs.push(("product-id", id.to_string()));
            // Inside a section, Shopify also records which section the form sits in.
            if let Value::Str(section_id) = ctx.find_variable("section").get("id") {
                shape
                    .trailing_inputs
                    .push(("section-id", section_id.to_string()));
            }
            shape
        }
        "cart" => {
            let mut shape = Shape::new(path("/cart"));
            shape.id = Some("cart_form".to_string());
            shape.class = Some("shopify-cart-form");
            shape.multipart = true;
            shape
        }
        "contact" | "customer" => {
            let mut shape = Shape::new(path("/contact"));
            shape.anchored = true;
            shape.id = Some("contact_form".to_string());
            shape.class = Some("contact-form");
            shape
        }
        "create_customer" => {
            let mut shape = Shape::new(path("/account"));
            shape.id = Some("create_customer".to_string());
            shape.extra_attributes = " data-login-with-shop-sign-up=\"true\"";
            shape
        }
        "customer_login" => {
            let mut shape = Shape::new(path("/account/login"));
            shape.id = Some("customer_login".to_string());
            shape.extra_attributes = " data-login-with-shop-sign-in=\"true\"";
            shape
        }
        "guest_login" => {
            let mut shape = Shape::new(path("/account/login"));
            shape.id = Some("customer_login_guest".to_string());
            shape.trailing_inputs.push(("guest", "true".to_string()));
            shape
        }
        "recover_customer_password" => Shape::new(path("/account/recover")),
        "reset_customer_password" => Shape::new(path("/account/reset")),
        "activate_customer_password" => Shape::new(path("/account/activate")),
        "customer_address" => match object.downcast::<AddressDrop>() {
            Some(address) => {
                let id = address.address.id;
                for key in [
                    "first_name",
                    "last_name",
                    "company",
                    "address1",
                    "address2",
                    "city",
                    "country",
                    "province",
                    "zip",
                    "phone",
                    "id",
                ] {
                    if let Some(value) = address.get(key) {
                        fields.insert(key.to_string(), value);
                    }
                }
                let mut shape = Shape::new(path(&format!("/account/addresses/{id}")));
                shape.id = Some(format!("address_form_{id}"));
                shape.leading_inputs.push(("_method", "put".to_string()));
                shape
            }
            None => {
                let mut shape = Shape::new(path("/account/addresses"));
                shape.id = Some("address_form_new".to_string());
                shape
            }
        },
        "new_comment" => {
            let Some(article) = object.downcast::<ArticleDrop>() else {
                return Err(Error::argument("new_comment form must be given an article"));
            };
            let mut shape = Shape::new(format!("{}/comments#comment_form", article.url()));
            shape.id = Some("comment_form".to_string());
            shape.class = Some("comment-form");
            shape
        }
        "localization" => {
            let mut shape = Shape::new(path("/localization"));
            shape.id = Some("localization_form".to_string());
            shape.class = Some("shopify-localization-form");
            shape.multipart = true;
            shape.leading_inputs.push(("_method", "put".to_string()));
            shape.leading_inputs.push(("return_to", current));
            shape
        }
        "currency" => {
            let mut shape = Shape::new(path("/cart/update"));
            shape.id = Some("currency_form".to_string());
            shape.class = Some("shopify-currency-form");
            shape.multipart = true;
            shape.leading_inputs.push(("return_to", current));
            shape
        }
        "storefront_password" => {
            let mut shape = Shape::new(path("/password"));
            shape.id = Some("login_form".to_string());
            shape.class = Some("storefront-password-form");
            shape
        }
        other => return Err(Error::argument(format!("Unknown form type '{other}'"))),
    };
    Ok((shape, fields))
}

impl Tag for Form {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let form_type = self.form_type.evaluate(ctx)?.to_str().into_owned();
        let object = match &self.object {
            Some(expr) => expr.evaluate(ctx)?,
            None => Value::Nil,
        };
        let mut attributes: Vec<(String, Value)> = Vec::new();
        for (key, expr) in &self.attributes {
            attributes.push((key.clone(), expr.evaluate(ctx)?));
        }
        let state = RenderState::of(ctx)?;
        let (mut shape, fields) = shape(ctx, state, &form_type, &object)?;

        let mut id = shape.id.clone();
        let mut class = shape.class.map(str::to_string);
        let mut custom = String::new();
        for (key, value) in &attributes {
            let text = value.to_str();
            match key.as_str() {
                "id" => id = Some(text.into_owned()),
                "class" => class = Some(text.into_owned()),
                "return_to" => shape.leading_inputs.push(("return_to", text.into_owned())),
                _ => custom.push_str(&format!(" {key}=\"{}\"", escape_html(&text))),
            }
        }

        if shape.anchored
            && let Some(id) = &id
        {
            shape.action = format!("{}#{id}", shape.action);
        }
        out.push_str(&format!(
            "<form method=\"post\" action=\"{}\"",
            escape_html(&shape.action)
        ));
        if let Some(id) = &id {
            out.push_str(&format!(" id=\"{}\"", escape_html(id)));
        }
        out.push_str(" accept-charset=\"UTF-8\"");
        if let Some(class) = &class {
            out.push_str(&format!(" class=\"{}\"", escape_html(class)));
        }
        if shape.multipart {
            out.push_str(" enctype=\"multipart/form-data\"");
        }
        out.push_str(shape.extra_attributes);
        out.push_str(&custom);
        out.push('>');
        out.push_str(&format!(
            "<input type=\"hidden\" name=\"form_type\" value=\"{}\" /><input type=\"hidden\" name=\"utf8\" value=\"✓\" />",
            escape_html(&form_type)
        ));
        for (name, value) in &shape.leading_inputs {
            out.push_str(&format!(
                "<input type=\"hidden\" name=\"{name}\" value=\"{}\" />",
                escape_html(value)
            ));
        }

        // The outcome of a submission is shown by the form of the same type.
        let result = state
            .site
            .session
            .form_result
            .clone()
            .filter(|result| result.form_type == form_type);
        let form = Value::object(FormDrop {
            form_type,
            id,
            result,
            fields,
        });
        ctx.push_scope()?;
        ctx.set("form", form);
        self.body.render(ctx, out);
        ctx.pop_scope();

        for (name, value) in &shape.trailing_inputs {
            out.push_str(&format!(
                "<input type=\"hidden\" name=\"{name}\" value=\"{}\" />",
                escape_html(value)
            ));
        }
        out.push_str("</form>");
        Ok(())
    }
}

pub(super) fn parse(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let mut markup = MarkupParser::new(token.markup)?;
    let form_type = Expr::parse(&markup.expression()?);
    let mut object = None;
    let mut attributes = Vec::new();
    while markup.consume_if(TokenKind::Comma).is_some() {
        if markup.look(TokenKind::Id) && markup.look_ahead(TokenKind::Colon, 1) {
            let key = markup.consume(TokenKind::Id)?.to_string();
            markup.consume(TokenKind::Colon)?;
            attributes.push((key, Expr::parse(&markup.expression()?)));
        } else if !markup.at_end() {
            object = Some(Expr::parse(&markup.expression()?));
        }
    }
    markup.consume(TokenKind::EndOfString)?;
    Ok(Box::new(Form {
        form_type,
        object,
        attributes,
        body: parser.parse_block("form")?,
    }))
}
