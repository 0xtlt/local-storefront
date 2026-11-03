//! `request`, `routes`, `template` and `theme`.

use std::any::Any;
use std::borrow::Cow;

use lsf_liquid::{Object, Value};

use super::SiteRef;
use super::localization::language_value;

pub struct RequestDrop {
    pub site: SiteRef,
    pub page_type: String,
}

impl Object for RequestDrop {
    fn type_name(&self) -> &str {
        "request"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let request = &self.site.request;
        Some(match key {
            "design_mode" | "visual_preview_mode" => Value::Bool(false),
            "page_type" => Value::from(&self.page_type),
            "host" => Value::from(&request.host),
            "origin" => Value::from(request.origin()),
            "path" => Value::from(request.localized(&request.path)),
            "locale" => language_value(&self.site, self.site.language()),
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct RoutesDrop {
    pub site: SiteRef,
}

/// The storefront paths behind each `routes` property.
pub const ROUTES: [(&str, &str); 19] = [
    ("root_url", "/"),
    ("account_url", "/account"),
    ("account_login_url", "/account/login"),
    ("account_logout_url", "/account/logout"),
    ("account_recover_url", "/account/recover"),
    ("account_register_url", "/account/register"),
    ("account_addresses_url", "/account/addresses"),
    ("account_profile_url", "/account/profile"),
    ("collections_url", "/collections"),
    ("all_products_collection_url", "/collections/all"),
    ("search_url", "/search"),
    ("predictive_search_url", "/search/suggest"),
    ("cart_url", "/cart"),
    ("cart_add_url", "/cart/add"),
    ("cart_change_url", "/cart/change"),
    ("cart_clear_url", "/cart/clear"),
    ("cart_update_url", "/cart/update"),
    ("product_recommendations_url", "/recommendations/products"),
    ("storefront_login_url", "/customer_authentication/login"),
];

impl Object for RoutesDrop {
    fn type_name(&self) -> &str {
        "routes"
    }

    fn get(&self, key: &str) -> Option<Value> {
        ROUTES
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, path)| Value::from(self.site.request.localized(path)))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The template being rendered: `product`, `page.contact`, `customers/login`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateName {
    pub directory: Option<String>,
    pub name: String,
    pub suffix: Option<String>,
}

impl TemplateName {
    pub fn new(name: &str) -> Self {
        TemplateName {
            directory: None,
            name: name.to_string(),
            suffix: None,
        }
    }

    /// `product.alternate`, `customers/login`.
    pub fn full(&self) -> String {
        let mut out = String::new();
        if let Some(directory) = &self.directory {
            out.push_str(directory);
            out.push('/');
        }
        out.push_str(&self.name);
        if let Some(suffix) = &self.suffix {
            out.push('.');
            out.push_str(suffix);
        }
        out
    }
}

pub struct TemplateDrop(pub TemplateName);

impl Object for TemplateDrop {
    fn type_name(&self) -> &str {
        "template"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "name" => Value::from(&self.0.name),
            "suffix" => self.0.suffix.as_ref().map_or(Value::Nil, Value::from),
            "directory" => self.0.directory.as_ref().map_or(Value::Nil, Value::from),
            _ => return None,
        })
    }

    /// `template` compares and prints as its full name, e.g. `{% if template contains 'product' %}`.
    fn to_value(&self) -> Option<Value> {
        Some(Value::from(self.0.full()))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(self.0.full())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
