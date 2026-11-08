//! Managed pricing in lsf's admin: the plan selection page Shopify hosts for an app, the page
//! where the merchant approves a plan, and the control endpoints of the plans. See
//! [`crate::admin::domains::app::pricing`] for what Shopify does and what lsf simulates of it.
//!
//! | Request | What answers |
//! |---|---|
//! | `GET /admin/charges/<app>/pricing_plans?shop=` | The plans, the shop's current one marked, each with a button to choose it; without plans, how to declare them. `<app>` is the app's handle or client id. |
//! | `GET /admin/charges/<app>/pricing_plans/<plan>?shop=` | The plan to approve: its price, its trial, the plan it replaces. |
//! | `GET /__lsf/admin/pricing-plans?shop=` | The plans, `legacy`, where they come from, the page, and the shop's current subscription. |
//! | `PUT /__lsf/admin/pricing-plans` (`POST` too) | Declares the plans: `{"plans": [...], "legacy"?}`, as `--pricing-plans` does. `422` for a wrong declaration. |
//! | `DELETE /__lsf/admin/pricing-plans` | Back to the plans of `--pricing-plans`, or none. |
//! | `POST /__lsf/admin/pricing-plans/<plan>/select` (`?shop=`, or `{"shop"}`) | The merchant chooses the plan: from the page's form, a `303` to its welcome link, as Shopify redirects; otherwise JSON `{ok, shop, plan, subscription, replaced, redirect}`. `404` for no such plan, `409` when the shop is on it already. |
//! | `POST /__lsf/admin/pricing-plans/cancel` (`?shop=`, or `{"shop"}`) | The shop's plan ends without the app doing it: its active subscription `CANCELLED`, `app_subscriptions/update` unless `legacy` is `false`. JSON `{ok, shop, plan, subscription, webhook}`. `409` when the shop has no active subscription. Shopify's page has no such button: a test's stand-in for a plan ended outside the app. |
//!
//! The page lives at Shopify's path in lsf's admin: a `shopify:admin/charges/<app>/pricing_plans`
//! link of the app comes here, and so does Shopify's own URL of it
//! (`https://admin.shopify.com/store/<store>/charges/<app>/pricing_plans`, which App Bridge opens
//! with `target: '_top'`): lsf's admin page maps it (`admin.js`).

use serde_json::{Value, json};

use super::admin::origin_of;
use super::app::shop_of;
use super::embedded::{
    Viewer, admin_config, admin_query, app_key, app_name, notice_page, render, with_query,
};
use super::{AdminApi, HttpRequest, HttpResponse, escape_html};
use crate::admin::domains::app::pricing::{self, Plan, Pricing, SelectError};
use crate::admin::state::ShopState;

/// Where the control endpoints of the plans are.
const CONTROL: &str = "/__lsf/admin/pricing-plans";

/// The path of the plan selection page of the app.
fn page_path(api: &AdminApi) -> String {
    format!("/admin/charges/{}/pricing_plans", app_key(api))
}

/// What the page says when no plan is declared.
fn how_to_declare() -> Vec<String> {
    vec![
        "The app declares no plans. On Shopify they are in the Partner Dashboard; lsf takes them from a file, --pricing-plans <file> (LSF_PRICING_PLANS), or from PUT /__lsf/admin/pricing-plans.".to_string(),
        "A plan has a handle, a name, a price (0 for a free plan), an interval (monthly or annual), trial days, features and a welcome link: see \"Managed pricing\" in lsf's admin guide.".to_string(),
    ]
}

// ---- the pages ----------------------------------------------------------------------------

/// `GET /admin/charges/<app>/pricing_plans[/<plan>]`.
pub(super) fn page(
    api: &AdminApi,
    request: &HttpRequest,
    viewer: &Viewer,
    app: &str,
    rest: &[&str],
) -> HttpResponse {
    let key = app_key(api);
    if app != key && app != api.client_id() {
        return notice_page(
            api,
            Some(viewer),
            404,
            "lsf has no such app",
            &[format!(
                "lsf runs one app, {}: its plans are at {}. /admin/charges/{app}/pricing_plans names no app lsf has.",
                app_name(api),
                page_path(api)
            )],
        );
    }
    let Some((pricing, _)) = api
        .pricing()
        .filter(|(pricing, _)| !pricing.plans.is_empty())
    else {
        return notice_page(
            api,
            Some(viewer),
            404,
            "No pricing plans",
            &how_to_declare(),
        );
    };
    let current = api.with_shop(&viewer.shop, pricing::current).flatten();
    match rest {
        [] => plans_page(api, request, viewer, &pricing, current.as_ref()),
        [handle] => match pricing.plan(handle) {
            Some(plan) => confirm_page(api, request, viewer, &pricing, plan, current.as_ref()),
            None => notice_page(
                api,
                Some(viewer),
                404,
                "There is no such plan",
                &[format!(
                    "{} has no plan {handle}: its plans are {}.",
                    app_name(api),
                    handles(&pricing)
                )],
            ),
        },
        _ => notice_page(
            api,
            Some(viewer),
            404,
            "lsf has no page here",
            &[format!(
                "The plans of {} are at {}.",
                app_name(api),
                page_path(api)
            )],
        ),
    }
}

fn handles(pricing: &Pricing) -> String {
    pricing
        .plans
        .iter()
        .map(|plan| plan.handle.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The admin around `main`, its title `title`.
fn admin_page(
    api: &AdminApi,
    request: &HttpRequest,
    viewer: &Viewer,
    title: &str,
    main: &str,
) -> HttpResponse {
    let config = admin_config(api, viewer, request);
    let base = format!("/admin/apps/{}", app_key(api));
    render(api, viewer, title, main, &Value::Object(config), &base, 200)
}

/// What a test subscription is called on the page: lsf's shops are development stores.
fn test_badge(pricing: &Pricing) -> &'static str {
    if pricing.legacy {
        " <span class=\"lsf-badge lsf-badge-info\" title=\"A development store's subscription is a test one: nothing is charged\">Test</span>"
    } else {
        ""
    }
}

fn trial_text(days: i64) -> String {
    format!("{days}-day free trial")
}

/// The plan selection page: a card per plan.
fn plans_page(
    api: &AdminApi,
    request: &HttpRequest,
    viewer: &Viewer,
    pricing: &Pricing,
    current: Option<&pricing::Current>,
) -> HttpResponse {
    let keep = admin_query(request);
    let mut main = String::from("<section class=\"lsf-plans\">");
    main.push_str(&format!(
        "<p class=\"lsf-plans-intro\">Choose a plan for <strong>{}</strong>.{}</p>",
        escape_html(&app_name(api)),
        match current {
            Some(current) if current.plan.is_none() => format!(
                " Your subscription now is {}, which the app made with the Billing API.",
                escape_html(&current.name)
            ),
            _ => String::new(),
        }
    ));
    main.push_str("<div class=\"lsf-plan-grid\">");
    for plan in &pricing.plans {
        let is_current =
            current.is_some_and(|current| current.plan.as_deref() == Some(&plan.handle));
        main.push_str(&format!(
            "<article class=\"lsf-plan{}\" data-plan=\"{}\"><div class=\"lsf-plan-head\"><h2>{}</h2>{}</div>",
            if is_current { " lsf-plan-current" } else { "" },
            escape_html(&plan.handle),
            escape_html(&plan.name),
            if is_current {
                "<span class=\"lsf-badge lsf-badge-success\">Current plan</span>"
            } else {
                ""
            }
        ));
        main.push_str(&format!(
            "<p class=\"lsf-plan-price\">{}</p>",
            price_html(plan)
        ));
        if plan.trial_days > 0 {
            main.push_str(&format!(
                "<p class=\"lsf-plan-trial\">{}</p>",
                escape_html(&trial_text(plan.trial_days))
            ));
        }
        if !plan.features.is_empty() {
            main.push_str("<ul class=\"lsf-plan-features\">");
            for feature in &plan.features {
                main.push_str(&format!("<li>{}</li>", escape_html(feature)));
            }
            main.push_str("</ul>");
        }
        if is_current {
            main.push_str(
                "<span class=\"lsf-button lsf-plan-choose\" aria-disabled=\"true\">Your plan</span>",
            );
        } else {
            let href = with_query(&format!("{}/{}", page_path(api), plan.handle), &keep);
            main.push_str(&format!(
                "<a class=\"lsf-button lsf-button-primary lsf-plan-choose\" href=\"{}\">Choose {}</a>",
                escape_html(&href),
                escape_html(&plan.name)
            ));
        }
        main.push_str("</article>");
    }
    main.push_str("</div>");
    main.push_str(&format!(
        "<p class=\"lsf-plans-note\">lsf's copy of the page Shopify hosts for an app on managed pricing: no money moves.{}</p>",
        if pricing.legacy {
            " The subscriptions of this development store are test ones; the app hears of each change (app_subscriptions/update) and gets charge_id and plan_handle on its welcome link."
        } else {
            " As for an app enrolled since April 2026: the welcome link gets plan_handle, and no webhook is sent."
        }
    ));
    main.push_str("</section>");
    admin_page(api, request, viewer, "Pricing plans", &main)
}

/// A plan's price, the amount first: `<strong>9.99 USD</strong> every 30 days`.
fn price_html(plan: &Plan) -> String {
    if plan.free {
        return "<strong>Free</strong>".to_string();
    }
    let text = plan.price_text();
    let (amount, every) = text
        .match_indices(' ')
        .nth(1)
        .map_or((text.as_str(), ""), |(at, _)| text.split_at(at));
    format!(
        "<strong>{}</strong>{}",
        escape_html(amount),
        escape_html(every)
    )
}

/// The page where the merchant approves a plan.
fn confirm_page(
    api: &AdminApi,
    request: &HttpRequest,
    viewer: &Viewer,
    pricing: &Pricing,
    plan: &Plan,
    current: Option<&pricing::Current>,
) -> HttpResponse {
    let keep = admin_query(request);
    let back = with_query(&page_path(api), &keep);
    let mut main = format!(
        "<section class=\"lsf-notice lsf-plan-confirm\" data-plan=\"{}\"><h1>{}{}</h1><p class=\"lsf-plan-price\">{}</p>",
        escape_html(&plan.handle),
        escape_html(&plan.name),
        if plan.free { "" } else { test_badge(pricing) },
        price_html(plan)
    );
    if plan.trial_days > 0 {
        main.push_str(&format!(
            "<p>{}, starting when you approve.</p>",
            escape_html(&trial_text(plan.trial_days))
        ));
    }
    let is_current = current.is_some_and(|current| current.plan.as_deref() == Some(&plan.handle));
    match current {
        Some(_) if is_current => {
            main.push_str(&format!(
                "<p>This is your plan.</p><p><a class=\"lsf-link\" href=\"{}\">Back to the plans</a></p></section>",
                escape_html(&back)
            ));
            return admin_page(api, request, viewer, "Pricing plans", &main);
        }
        Some(current) => main.push_str(&format!(
            "<p>It replaces your current plan, {}, at once.</p>",
            escape_html(&current.name)
        )),
        None => {}
    }
    let action = with_query(
        &format!("{CONTROL}/{}/select", plan.handle),
        &[("shop".to_string(), viewer.shop.clone())],
    );
    main.push_str(&format!(
        "<form class=\"lsf-plan-actions\" method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"shop\" value=\"{}\">\
         <a class=\"lsf-button\" href=\"{}\">Back</a><button class=\"lsf-button lsf-button-primary\" type=\"submit\">{}</button></form>",
        escape_html(&action),
        escape_html(&viewer.shop),
        escape_html(&back),
        if plan.free { "Choose" } else { "Approve" }
    ));
    let link = plan.welcome_link.as_deref().unwrap_or("/");
    main.push_str(&format!(
        "<p>Then you go to the app's welcome link, {}, with {}. No money moves: lsf simulates the approval Shopify asks of the merchant.</p></section>",
        escape_html(link),
        if pricing.legacy {
            "charge_id and plan_handle"
        } else {
            "plan_handle"
        }
    ));
    let title = if plan.free {
        "Choose a plan"
    } else {
        "Approve subscription"
    };
    admin_page(api, request, viewer, title, &main)
}

// ---- the control endpoints ----------------------------------------------------------------

/// `/__lsf/admin/pricing-plans[/<plan>/select]`.
pub(super) fn control(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    let parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    match (method.as_str(), parts.as_slice()) {
        ("GET", []) => HttpResponse::json(200, &status(api, &shop_of(api, request))),
        ("PUT" | "POST", []) => {
            let body = String::from_utf8_lossy(&request.body);
            let value: Value = match serde_json::from_str(&body) {
                Ok(value) => value,
                Err(error) => {
                    return HttpResponse::error(
                        400,
                        format!("the body is JSON, {{\"plans\": [...], \"legacy\"?}}: {error}"),
                    );
                }
            };
            match Pricing::from_json(&value) {
                Ok(pricing) => {
                    api.set_pricing(Some(pricing));
                    let mut answer = status(api, &shop_of(api, request));
                    answer["ok"] = json!(true);
                    HttpResponse::json(200, &answer)
                }
                Err(error) => HttpResponse::error(422, error),
            }
        }
        ("DELETE", []) => {
            api.set_pricing(None);
            let mut answer = status(api, &shop_of(api, request));
            answer["ok"] = json!(true);
            HttpResponse::json(200, &answer)
        }
        ("POST", ["cancel"]) => cancel(api, request),
        ("POST", [plan, "select"]) => select(api, request, plan),
        (_, [] | ["cancel"] | [_, "select"]) => HttpResponse::not_allowed(&method),
        _ => HttpResponse::error(404, format!("no control endpoint at {CONTROL}/{rest}")),
    }
}

/// What `GET /__lsf/admin/pricing-plans` answers.
fn status(api: &AdminApi, shop: &str) -> Value {
    let (pricing, source) = match api.pricing() {
        Some((pricing, source)) => ((*pricing).clone(), Some(source)),
        None => (Pricing::default(), None),
    };
    let current = api.with_shop(shop, pricing::current).flatten();
    let mut answer = pricing.to_json();
    answer["source"] = json!(source);
    answer["managedPricing"] = json!(!pricing.plans.is_empty());
    answer["page"] = json!(page_path(api));
    answer["shop"] = json!(shop);
    answer["current"] = match current {
        Some(current) => json!({"id": current.id, "name": current.name, "plan": current.plan}),
        None => Value::Null,
    };
    answer
}

/// What the answer says of a subscription.
fn subscription_json(state: &ShopState, id: &str, number: u64) -> Value {
    let record = state.get(id);
    let field = |name: &str| {
        record
            .and_then(|record| record.get(name))
            .cloned()
            .unwrap_or(Value::Null)
    };
    json!({
        "id": id,
        "chargeId": number,
        "name": field("name"),
        "status": field("status"),
        "test": field("test"),
        "trialDays": field("trialDays"),
        "trialEndsAt": field("trialEndsAt"),
        "currentPeriodEnd": field("currentPeriodEnd"),
    })
}

/// `POST /__lsf/admin/pricing-plans/cancel`: the shop's plan ends without the app doing it
/// ([`pricing::cancel`]).
fn cancel(api: &AdminApi, request: &HttpRequest) -> HttpResponse {
    let shop = shop_of(api, request);
    let Some((pricing, _)) = api
        .pricing()
        .filter(|(pricing, _)| !pricing.plans.is_empty())
    else {
        return HttpResponse::error(404, how_to_declare().join(" "));
    };
    let ended = api.change_shop(&shop, |state| {
        pricing::cancel(state, &pricing).map(|ended| {
            let subscription = subscription_json(state, &ended.subscription, ended.number);
            (ended, subscription)
        })
    });
    match ended {
        None => HttpResponse::error(404, format!("there is no shop {shop}")),
        Some(None) => HttpResponse::error(
            409,
            format!("{shop} has no active subscription: no plan to end"),
        ),
        Some(Some((ended, subscription))) => HttpResponse::json(
            200,
            &json!({
                "ok": true,
                "shop": shop,
                "plan": ended.plan.as_deref().and_then(|handle| pricing.plan(handle)).map(Plan::to_json),
                "subscription": subscription,
                "webhook": ended.webhook,
            }),
        ),
    }
}

/// `POST /__lsf/admin/pricing-plans/<plan>/select`: the merchant chooses a plan.
fn select(api: &AdminApi, request: &HttpRequest, handle: &str) -> HttpResponse {
    let shop = shop_of(api, request);
    let Some((pricing, _)) = api
        .pricing()
        .filter(|(pricing, _)| !pricing.plans.is_empty())
    else {
        return HttpResponse::error(404, how_to_declare().join(" "));
    };
    let admin_app = format!("{}/admin/apps/{}", origin_of(request), app_key(api));
    let chosen = api.change_shop(&shop, |state| {
        pricing::select(state, &pricing, handle, |plan, number| {
            pricing::welcome_url(plan, &admin_app, &shop, pricing.legacy, number)
        })
        .map(|selected| {
            let subscription = subscription_json(state, &selected.subscription, selected.number);
            (selected, subscription)
        })
    });
    let from_page = request
        .header("content-type")
        .is_some_and(|kind| kind.starts_with("application/x-www-form-urlencoded"));
    match chosen {
        None => HttpResponse::error(404, format!("there is no shop {shop}")),
        Some(Err(SelectError::UnknownPlan)) => HttpResponse::error(
            404,
            format!(
                "there is no plan {handle}: the plans are {}",
                handles(&pricing)
            ),
        ),
        Some(Err(SelectError::Current)) => {
            HttpResponse::error(409, format!("{shop} is on the plan {handle} already"))
        }
        Some(Ok((selected, _))) if from_page => HttpResponse {
            status: 303,
            headers: vec![("location".into(), selected.redirect)],
            body: Vec::new(),
        },
        Some(Ok((selected, subscription))) => HttpResponse::json(
            200,
            &json!({
                "ok": true,
                "shop": shop,
                "plan": pricing.plan(handle).map(Plan::to_json),
                "subscription": subscription,
                "replaced": selected.replaced,
                "redirect": selected.redirect,
            }),
        ),
    }
}
