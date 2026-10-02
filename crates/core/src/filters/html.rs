//! Filters that generate HTML tags and snippets.

use lsf_liquid::filters::escape_html;
use lsf_liquid::{Context, Environment, FilterArgs, Result, Value};

use super::{html_attributes, site};
use crate::drops::shop::AddressDrop;
use crate::render::state::RenderState;
use crate::store::reference;
use crate::tags::FormDrop;

fn stylesheet_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let url = input.to_str();
    if args.named("preload").is_some_and(Value::is_truthy) {
        RenderState::of(ctx)?.add_preload(format!("<{url}>; rel=preload; as=style"));
    }
    let media = args
        .named("media")
        .map(|media| media.to_str().into_owned())
        .unwrap_or_else(|| "all".to_string());
    Ok(Value::from(format!(
        "<link href=\"{}\" rel=\"stylesheet\" type=\"text/css\" media=\"{}\"{} />",
        escape_html(&url),
        escape_html(&media),
        html_attributes(&args.named, &["preload", "media"])
    )))
}

fn script_tag(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let kind = args
        .named("type")
        .map(|kind| kind.to_str().into_owned())
        .unwrap_or_else(|| "text/javascript".to_string());
    Ok(Value::from(format!(
        "<script src=\"{}\" type=\"{}\"{}></script>",
        escape_html(&input.to_str()),
        escape_html(&kind),
        html_attributes(&args.named, &["type"])
    )))
}

fn preload_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let url = input.to_str();
    let kind = args
        .named("as")
        .map(|kind| kind.to_str().into_owned())
        .unwrap_or_default();
    RenderState::of(ctx)?.add_preload(format!("<{url}>; rel=preload; as={kind}"));
    Ok(Value::from(format!(
        "<link href=\"{}\"{} rel=\"preload\">",
        escape_html(&url),
        html_attributes(&args.named, &[])
    )))
}

/// A neutral placeholder illustration. Shopify draws themed illustrations (a product, a
/// lifestyle scene, ...); locally a simple shape of the same proportions stands in.
fn placeholder_svg_tag(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let name = input.to_str();
    let class = args
        .get(0)
        .map(|class| format!(" class=\"{}\"", escape_html(&class.to_str())))
        .unwrap_or_default();
    let wide = name.starts_with("hero")
        || name.starts_with("lifestyle")
        || name.starts_with("blog")
        || name == "image";
    let (view_box, shapes) = if wide {
        (
            "0 0 1052 400",
            "<rect width=\"1052\" height=\"400\" fill=\"#f1f1f1\"/><path d=\"M0 300 260 150l180 110 200-160 412 300H0z\" fill=\"#dcdcdc\"/><circle cx=\"840\" cy=\"100\" r=\"42\" fill=\"#dcdcdc\"/>",
        )
    } else {
        (
            "0 0 525.5 525.5",
            "<rect width=\"525.5\" height=\"525.5\" fill=\"#f1f1f1\"/><path d=\"M0 420 150 250l110 100 110-140 155.5 210v105.5H0z\" fill=\"#dcdcdc\"/><circle cx=\"390\" cy=\"140\" r=\"40\" fill=\"#dcdcdc\"/>",
        )
    };
    Ok(Value::from(format!(
        "<svg{class} xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{view_box}\" data-placeholder=\"{}\">{shapes}</svg>",
        escape_html(&name)
    )))
}

fn inline_asset_content(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let name = input.to_str();
    Ok(
        match site(ctx)?.theme.files().read(&format!("assets/{name}")) {
            Some(content) => Value::Str(content),
            None => {
                ctx.warn(format!(
                    "inline_asset_content: assets/{name} does not exist"
                ));
                Value::empty_string()
            }
        },
    )
}

/// The display name of a payment type handle.
fn payment_type_name(handle: &str) -> String {
    match handle {
        "master" => "Mastercard".to_string(),
        "american_express" => "American Express".to_string(),
        "diners_club" => "Diners Club".to_string(),
        "shopify_pay" => "Shop Pay".to_string(),
        "apple_pay" => "Apple Pay".to_string(),
        "google_pay" => "Google Pay".to_string(),
        "paypal" => "PayPal".to_string(),
        "jcb" => "JCB".to_string(),
        other => crate::util::humanize(other),
    }
}

/// A generic payment badge carrying the payment type's name.
pub fn payment_icon_svg(handle: &str, class: Option<&str>) -> String {
    let name = payment_type_name(handle);
    let class = class
        .map(|class| format!(" class=\"{}\"", escape_html(class)))
        .unwrap_or_default();
    format!(
        "<svg{class} xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" viewBox=\"0 0 38 24\" width=\"38\" height=\"24\" aria-labelledby=\"pi-{handle}\"><title id=\"pi-{handle}\">{}</title><rect x=\".5\" y=\".5\" width=\"37\" height=\"23\" rx=\"2.5\" fill=\"#fff\" stroke=\"#000\" stroke-opacity=\".2\"/><text x=\"19\" y=\"14.5\" font-family=\"sans-serif\" font-size=\"5.5\" font-weight=\"700\" text-anchor=\"middle\" fill=\"#333\">{}</text></svg>",
        escape_html(&name),
        escape_html(&name.to_uppercase())
    )
}

fn payment_type_svg_tag(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let class = args.named("class").map(|class| class.to_str().into_owned());
    Ok(Value::from(payment_icon_svg(
        &input.to_str(),
        class.as_deref(),
    )))
}

fn default_pagination(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let anchor = args
        .named("anchor")
        .map(|anchor| format!("#{}", anchor.to_str()))
        .unwrap_or_default();
    let link = |part: &Value, title: &str| {
        format!(
            "<a href=\"{}{anchor}\" title=\"\">{title}</a>",
            escape_html(&part.get("url").to_str())
        )
    };
    let mut pieces = Vec::new();
    let previous = input.get("previous");
    if previous.is_truthy() {
        let title = args
            .named("previous")
            .map(|t| t.to_str().into_owned())
            .unwrap_or_else(|| previous.get("title").to_str().into_owned());
        pieces.push(format!(
            "<span class=\"prev\">{}</span>",
            link(&previous, &title)
        ));
    }
    for part in input.get("parts").items().unwrap_or_default().iter() {
        let title = part.get("title").to_str().into_owned();
        pieces.push(if part.get("is_link").is_truthy() {
            format!("<span class=\"page\">{}</span>", link(part, &title))
        } else if title == "&hellip;" {
            "<span class=\"deco\">&hellip;</span>".to_string()
        } else {
            format!("<span class=\"page current\">{title}</span>")
        });
    }
    let next = input.get("next");
    if next.is_truthy() {
        let title = args
            .named("next")
            .map(|t| t.to_str().into_owned())
            .unwrap_or_else(|| next.get("title").to_str().into_owned());
        pieces.push(format!(
            "<span class=\"next\">{}</span>",
            link(&next, &title)
        ));
    }
    Ok(Value::from(pieces.join(" ")))
}

fn default_errors(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let Some(fields) = input.items().filter(|fields| !fields.is_empty()) else {
        return Ok(Value::empty_string());
    };
    let messages = input.get("messages");
    let translated = input.get("translated_fields");
    let mut out = String::from("<div class=\"errors\"><ul>");
    for field in fields.iter() {
        let key = field.to_str();
        let message = messages.get(&key).to_str().into_owned();
        // `form` is the pseudo-field of errors that do not belong to one field.
        if key == "form" {
            out.push_str(&format!("<li>{}</li>", escape_html(&message)));
        } else {
            out.push_str(&format!(
                "<li>{} {}</li>",
                escape_html(&translated.get(&key).to_str()),
                escape_html(&message)
            ));
        }
    }
    out.push_str("</ul></div>");
    Ok(Value::from(out))
}

fn payment_button(_input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::str(
        "<div data-shopify=\"payment-button\" class=\"shopify-payment-button\"><button type=\"button\" class=\"shopify-payment-button__button shopify-payment-button__button--unbranded\">Buy it now</button></div>",
    ))
}

fn empty(_input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::empty_string())
}

fn format_address(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<AddressDrop>() else {
        return Ok(Value::empty_string());
    };
    let address = &drop.address;
    let name = [address.first_name.as_str(), address.last_name.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let locality = [
        address.city.as_str(),
        address.province_code.as_str(),
        address.zip.as_str(),
    ]
    .iter()
    .filter(|part| !part.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(" ");
    let lines = [
        name.as_str(),
        address.company.as_str(),
        address.address1.as_str(),
        address.address2.as_str(),
        locality.as_str(),
        address.country.as_str(),
    ];
    let html: Vec<String> = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| escape_html(line))
        .collect();
    Ok(Value::from(format!("<p>{}</p>", html.join("<br>"))))
}

fn currency_selector(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    if input.downcast::<FormDrop>().is_none() {
        return Ok(Value::empty_string());
    }
    let site = site(ctx)?;
    let mut codes: Vec<&str> = vec![&site.store.shop.currency];
    for country in &site.store.countries {
        if !codes.contains(&country.currency.as_str()) {
            codes.push(&country.currency);
        }
    }
    codes.sort_unstable();
    let options: String = codes
        .iter()
        .map(|code| {
            let selected = if *code == site.currency() {
                " selected=\"selected\""
            } else {
                ""
            };
            format!(
                "<option value=\"{code}\"{selected}>{code} {}</option>",
                reference::currency_symbol(code)
            )
        })
        .collect();
    Ok(Value::from(format!(
        "<select{} name=\"currency\">{options}</select>",
        html_attributes(&args.named, &[])
    )))
}

/// Wraps every occurrence of the search terms in `<strong class="highlight">`.
fn highlight(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let text = input.to_str();
    let terms = args.at(0).to_str().into_owned();
    let mut out = text.into_owned();
    for term in terms.split_whitespace() {
        let lower = out.to_lowercase();
        let needle = term.to_lowercase();
        if needle.is_empty() || lower.len() != out.len() {
            continue;
        }
        let mut result = String::with_capacity(out.len());
        let mut position = 0;
        while let Some(found) = lower[position..].find(&needle) {
            let start = position + found;
            let end = start + needle.len();
            result.push_str(&out[position..start]);
            result.push_str(&format!(
                "<strong class=\"highlight\">{}</strong>",
                &out[start..end]
            ));
            position = end;
        }
        result.push_str(&out[position..]);
        out = result;
    }
    Ok(Value::from(out))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("stylesheet_tag", stylesheet_tag);
    env.register_filter("script_tag", script_tag);
    env.register_filter("preload_tag", preload_tag);
    env.register_filter("placeholder_svg_tag", placeholder_svg_tag);
    env.register_filter("inline_asset_content", inline_asset_content);
    env.register_filter("payment_type_svg_tag", payment_type_svg_tag);
    env.register_filter("default_pagination", default_pagination);
    env.register_filter("default_errors", default_errors);
    env.register_filter("payment_button", payment_button);
    env.register_filter("payment_terms", empty);
    env.register_filter("login_button", empty);
    env.register_filter("avatar", empty);
    env.register_filter("format_address", format_address);
    env.register_filter("currency_selector", currency_selector);
    env.register_filter("highlight", highlight);
}
