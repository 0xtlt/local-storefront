//! The local checkout of a Shopify app: the pages at `/checkouts/cn/<token>`, the operations
//! the page sends, and the files of the page and of the checkout UI extensions.
//!
//! What the buyer chose lives in the session (`SessionEntry::checkout`), next to the cart, which
//! the checkout shares with the theme. Every request computes the checkout again, running the
//! Functions of the app: `lsf_checkout::pipeline::compute`.

use std::sync::Arc;

use indexmap::IndexMap;
use lsf_checkout::app::{App, Selection};
use lsf_checkout::pipeline::{self, DELIVERY_GROUP_HANDLE};
use lsf_checkout::{
    Address, BuyerJourneyStep, CheckoutResult, CheckoutState, Metafield, ResultLine,
};
use lsf_core::drops::cart::line_key;
use lsf_core::store::model::ScenarioLook;
use lsf_core::store::{CartLine, Customer, Language};
use lsf_core::{Session, Store};
use serde_json::{Value as Json, json};

use super::checkout_account::{self, Identity};
use super::reply::{Reply, cache, content_type};
use super::{Incoming, SESSION_COOKIE, ServerState, SessionEntry, cart_token_of, home};
use crate::checkout::{
    CheckoutOptions, Variant, config_for, entered_by, scenario_named, selection_for, translations,
};

/// Where the files of the page are served from.
const ASSETS: &str = "/__lsf/checkout/assets";

/// What the checkout keeps for a session, besides the cart.
#[derive(Default)]
pub struct CheckoutSession {
    /// What the buyer entered and chose.
    pub state: CheckoutState,
    /// The look the buyer asked for with `?variant=`.
    pub variant: Option<Variant>,
    /// The state of the order the buyer completed, which the thank-you page shows.
    pub completed: Option<Json>,
    /// How many orders this session placed: a completion that started before one finished
    /// must not place another.
    pub placed: u64,
    /// Who the checkout was last prepared for: when the session has another customer, what the
    /// checkout holds of the one before gives way (see `checkout_account::prepare`).
    pub prepared: Option<Identity>,
}

/// A store, which is the same as another only if it is the very one that was loaded.
struct SameStore(Arc<Store>);

impl PartialEq for SameStore {
    fn eq(&self, other: &SameStore) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a request read of the session: everything `compute` runs the Functions on. A completion
/// may only consume the cart, and an update only write, if this is still what the session holds,
/// otherwise the buyer would pay for something they did not see, and wipe or restore what
/// changed meanwhile (another tab, another completion, a login, a reload of the data).
#[derive(PartialEq)]
struct Basis {
    cart_lines: Vec<CartLine>,
    cart_note: String,
    cart_attributes: IndexMap<String, String>,
    customer_id: Option<u64>,
    company_location: Option<u64>,
    country: Option<String>,
    /// The store the session sees: its own, or the loaded one. Another one when the data was
    /// reloaded or the control API gave the session a store.
    store: SameStore,
    state: Json,
    placed: u64,
}

impl Basis {
    /// `loaded` is the store of the server, which a session without a store of its own sees.
    fn of(entry: &SessionEntry, loaded: &Arc<Store>) -> Basis {
        Basis {
            cart_lines: entry.session.cart_lines.clone(),
            cart_note: entry.session.cart_note.clone(),
            cart_attributes: entry.session.cart_attributes.clone(),
            customer_id: entry.session.customer_id,
            company_location: entry.session.company_location,
            country: entry.session.country.clone(),
            store: SameStore(entry.store.clone().unwrap_or_else(|| loaded.clone())),
            state: json!(entry.checkout.state),
            placed: entry.checkout.placed,
        }
    }
}

/// The pages and the API of the checkout. `None` when the request is not for the checkout, so
/// that the storefront answers it.
pub fn route(state: &ServerState, incoming: &Incoming) -> Option<Reply> {
    let options = state.options.checkout.as_ref()?;
    // The theme's cart form leads to `/fr/checkout` in French.
    // With the languages of the store the session sees, as the storefront does.
    let (_, _, path) = state.localize(&state.store_of(incoming), &incoming.path);
    let path = path.trim_matches('/');
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    let method = incoming.method.as_str();
    let reply = match (method, segments.as_slice()) {
        ("GET" | "HEAD", ["checkout"]) => Some(start(state, incoming)),
        ("GET" | "HEAD", ["checkouts", "cn", token]) => Some(match token.strip_suffix(".json") {
            Some(token) => state_json(state, incoming, token),
            None => page(state, incoming, token),
        }),
        ("GET" | "HEAD", ["checkouts", "cn", token, "thank-you"]) => {
            Some(thank_you(state, incoming, token))
        }
        ("POST", ["checkouts", "cn", token, "update"]) => Some(update(state, incoming, token)),
        ("POST", ["checkouts", "cn", token, "complete"]) => Some(complete(state, incoming, token)),
        ("GET" | "HEAD", []) if options.standalone => Some(home(state, incoming)),
        _ if options.standalone => Some(Reply::text(
            404,
            "Not found. This server shows the checkout of an app only: start `lsf serve --app` \
             to serve a theme with it.",
        )),
        _ => None,
    }?;
    Some(reply)
}

/// The API under `/__lsf` that belongs to the checkout: `segments` follow `/__lsf/`.
pub fn control(state: &ServerState, incoming: &Incoming, segments: &[&str]) -> Option<Reply> {
    state.options.checkout.as_ref()?;
    match (incoming.method.as_str(), segments) {
        ("GET", ["checkout"]) => Some(control_state(state, incoming)),
        ("GET", ["checkouts"]) => Some(home(state, incoming)),
        ("GET", ["checkouts", "check"]) => Some(home_check(state, incoming)),
        ("GET", ["checkout", "assets", file]) => Some(asset(file)),
        ("GET", ["extensions", handle, file @ ..]) if !file.is_empty() => {
            Some(extension_file(state, handle, file))
        }
        _ => None,
    }
}

fn options(state: &ServerState) -> &CheckoutOptions {
    state
        .options
        .checkout
        .as_ref()
        .expect("checkout routes need --app")
}

fn error(status: u16, message: impl Into<String>) -> Reply {
    Reply::json(status, &json!({ "error": message.into() })).cached(cache::PRIVATE)
}

/// Gives a new visitor the session cookie, as the storefront does.
fn with_cookie(reply: Reply, session_id: &str, is_new: bool) -> Reply {
    if is_new {
        reply.header(
            "set-cookie",
            format!("{SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Lax"),
        )
    } else {
        reply
    }
}

/// The session of the request, when the token in its URL is the one of the session's cart.
fn session_of(
    state: &ServerState,
    incoming: &Incoming,
    token: &str,
) -> Result<(String, bool), Reply> {
    let (id, is_new) = state.session_id(incoming);
    let known = state.with_session(&id, |entry| entry.session.cart_token.clone());
    if known == token {
        Ok((id, is_new))
    } else {
        Err(error(
            404,
            format!("There is no checkout with the token {token}."),
        ))
    }
}

fn token_of(state: &ServerState, session_id: &str) -> String {
    state.with_session(session_id, |entry| entry.session.cart_token.clone())
}

/// The path of a checkout under the locale root of the buyer's language (`/fr`, nothing for
/// the primary language), as the storefront prefixes its own paths.
fn checkout_path(root: &str, token: &str) -> String {
    format!("{root}/checkouts/cn/{token}")
}

/// The language the buyer reads the checkout in: the one the locale prefix of the URL names
/// (`/fr/checkouts/cn/<token>`), otherwise the primary language of the store. The control API
/// has no prefix to read, so it takes `?locale=`.
fn language_of<'a>(state: &ServerState, store: &'a Store, incoming: &Incoming) -> &'a Language {
    let (iso_code, _, _) = state.localize(store, &incoming.path);
    let asked = incoming
        .path
        .starts_with("/__lsf")
        .then(|| incoming.query_param("locale"))
        .flatten();
    store
        .language(asked.unwrap_or(&iso_code))
        .unwrap_or_else(|| store.primary_language())
}

/// The locale root of a language, which is empty for the primary one.
fn root_of(language: &Language) -> &str {
    if language.primary {
        ""
    } else {
        language.root_url.trim_end_matches('/')
    }
}

/// The locale root of the request, for a session.
fn request_root(state: &ServerState, session_id: &str, incoming: &Incoming) -> String {
    let (_, store) = state.snapshot(session_id);
    root_of(language_of(state, &store, incoming)).to_string()
}

/// The home page: the extensions of the app and the checkout scenarios. It is the page of `/` of
/// a checkout without a theme; with a theme, which owns `/`, it is `/__lsf/checkouts`.
fn home(state: &ServerState, incoming: &Incoming) -> Reply {
    let store = state.store_of(incoming);
    let language = language_of(state, &store, incoming);
    let root = root_of(language).to_string();
    let check = home::check_url((!language.primary).then_some(language.iso_code.as_str()));
    home::page(state, incoming, options(state), &root, &check)
}

/// `/__lsf/checkouts/check`: the Functions that fail when the scenarios run, for the home page.
fn home_check(state: &ServerState, incoming: &Incoming) -> Reply {
    let store = state.store_of(incoming);
    let root = root_of(language_of(state, &store, incoming)).to_string();
    home::check(state, incoming, options(state), &root)
}

/// `/checkout`: a checkout for the cart of the visitor, in the language of the request. With
/// `?scenario=<name>`, a new one set up as that scenario says.
fn start(state: &ServerState, incoming: &Incoming) -> Reply {
    if let Some(name) = incoming.query_param("scenario") {
        return open_scenario(state, incoming, name);
    }
    let (id, is_new) = state.session_id(incoming);
    if let Some(who) = incoming.query_param("customer") {
        let (_, store) = state.snapshot(&id);
        let customer = match customer_of_query(state, &store, incoming, who) {
            Ok(customer) => customer,
            Err(reply) => return with_cookie(reply, &id, is_new),
        };
        // The login of the menu, on the checkout as it is.
        state.with_session(&id, |entry| {
            let checkout = &mut entry.checkout;
            checkout_account::prepare(
                &store,
                &mut entry.session,
                &mut checkout.state,
                &mut checkout.prepared,
            );
            checkout_account::switch_to(
                &store,
                &mut entry.session,
                &mut checkout.state,
                customer,
                None,
            );
            checkout.prepared = Some(Identity::of(&store, &entry.session));
        });
    }
    let root = request_root(state, &id, incoming);
    let mut location = checkout_path(&root, &token_of(state, &id));
    location.push_str(&carried_query(incoming, &["variant", "targets"]));
    with_cookie(Reply::redirect(&location), &id, is_new)
}

/// The customer `?customer=` logs the checkout in as: an email, `default`, or `none` for a guest.
/// Someone the store data does not have is a page that lists who it has.
fn customer_of_query(
    state: &ServerState,
    store: &Store,
    incoming: &Incoming,
    who: &str,
) -> Result<Option<u64>, Reply> {
    store
        .customer_named(who)
        .map(|customer| customer.map(|customer| customer.id))
        .map_err(|problem| {
            let known: Vec<&str> = store
                .customers
                .iter()
                .map(|customer| customer.email.as_str())
                .collect();
            let hint = if known.is_empty() {
                "The store data has no customers: add some to `customers.json`.".to_string()
            } else {
                format!("The customers are: {}.", known.join(", "))
            };
            let language = language_of(state, store, incoming);
            home::notice(
                404,
                &format!("No customer {who}"),
                &[problem, hint],
                &[],
                &home_path(state, root_of(language)),
            )
        })
}

/// The query parameters of the request that the checkout page keeps, as `?a=1&b=2`.
fn carried_query(incoming: &Incoming, names: &[&str]) -> String {
    let pairs: Vec<String> = names
        .iter()
        .filter_map(|name| {
            incoming
                .query_param(name)
                .map(|value| format!("{name}={}", escape_query(value)))
        })
        .collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

/// The page that lists the scenarios, for the notices to lead back to.
fn home_path(state: &ServerState, root: &str) -> String {
    if options(state).standalone {
        format!("{root}/")
    } else {
        "/__lsf/checkouts".to_string()
    }
}

/// `/checkout?scenario=<name>`: a new session set up as the scenario says, and a redirect to
/// its checkout. The session is a new one, with a token of its own, so that nothing the buyer
/// did in the checkout before (what the extensions stored, an order) is in it; the browser
/// keeps that session instead of the one it had.
fn open_scenario(state: &ServerState, incoming: &Incoming, name: &str) -> Reply {
    let loaded = state.loaded();
    let store = &loaded.store;
    let options = options(state);
    let request_language = language_of(state, store, incoming);
    let home = home_path(state, root_of(request_language));
    let notice = |status, title: &str, paragraphs: &[String], details: &[String]| {
        home::notice(status, title, paragraphs, details, &home)
    };

    let Some(scenario) = store.scenario(name) else {
        let known: Vec<&str> = store
            .scenarios
            .iter()
            .map(|scenario| scenario.name.as_str())
            .collect();
        let hint = if known.is_empty() {
            "There are no scenarios: `lsf checkout init` writes a set in the `checkouts` folder of the store data.".to_string()
        } else {
            format!("The scenarios are: {}.", known.join(", "))
        };
        return notice(404, &format!("No scenario named {name}"), &[hint], &[]);
    };
    if !scenario.problems.is_empty() {
        return notice(
            422,
            &format!("The scenario {name} has errors"),
            &[format!("Fix {} and reload.", scenario.file)],
            &scenario.problems,
        );
    }
    let app = match lsf_checkout::app::load(&options.app_dir) {
        Ok(app) => app,
        Err(problem) => return notice(500, "The app cannot be read", &[problem], &[]),
    };
    if let Err(problem) = selection_for(&app, &options.extensions, Some(scenario)) {
        return notice(
            422,
            &format!("The scenario {name} has errors"),
            &[format!("{}: {problem}", scenario.file)],
            &[],
        );
    }

    let id = state.new_session_id();
    let (mut session, mut entered, mut prepared) = match scenario_start(state, store, scenario) {
        Ok(start) => start,
        Err(details) => {
            return notice(
                422,
                &format!("The scenario {name} has errors"),
                &[],
                &details,
            );
        }
    };
    // `&customer=` logs the scenario's checkout in as someone else, as the menu would once it
    // is open: checked before anything is set up.
    let login = match incoming.query_param("customer") {
        Some(who) => match customer_of_query(state, store, incoming, who) {
            Ok(customer) => Some(customer),
            Err(reply) => return reply,
        },
        None => None,
    };
    if let Some(customer) = login {
        checkout_account::switch_to(store, &mut session, &mut entered, customer, None);
        prepared = Some(Identity::of(store, &session));
    }
    let token = cart_token_of(&id);
    session.cart_token = token.clone();
    let look = scenario.look.map(|look| match look {
        ScenarioLook::Standard => Variant::Standard,
        ScenarioLook::Shop => Variant::Shop,
    });
    state.with_session(&id, |entry| {
        entry.session = session;
        entry.checkout = CheckoutSession {
            state: entered,
            variant: look,
            prepared,
            ..CheckoutSession::default()
        };
    });

    // Under the locale root of the scenario's language, otherwise of the request's.
    let language = scenario
        .language
        .as_deref()
        .and_then(|iso| store.language(iso))
        .unwrap_or(request_language);
    let mut location = checkout_path(root_of(language), &token);
    location.push_str(&carried_query(incoming, &["variant", "targets"]));
    with_cookie(Reply::redirect(&location), &id, true)
}

/// The session and the checkout a scenario opens with, as the first read prepares them, and
/// who they were prepared for. `Err` holds what is wrong with its cart or buyer.
fn scenario_start(
    state: &ServerState,
    store: &Store,
    scenario: &lsf_core::store::CheckoutScenario,
) -> Result<(Session, CheckoutState, Option<Identity>), Vec<String>> {
    let mut session = state.initial_session(store);
    let mut problems = lsf_core::diagnostics::Diagnostics::new();
    lsf_core::store::build::apply_session_input(
        store,
        &scenario.session,
        &mut session,
        &mut problems,
        &scenario.file,
        "/session",
    );
    if problems.has_errors() {
        return Err(problems.items.iter().map(ToString::to_string).collect());
    }
    let mut entered = entered_by(scenario);
    // As entering the address does: markets decide on the country of the address.
    if let Some(code) = entered
        .shipping_address
        .as_ref()
        .and_then(|address| address.country_code.as_deref())
        .and_then(|code| store.country(code))
    {
        session.country = Some(code.iso_code.clone());
    }
    let mut prepared = None;
    checkout_account::prepare(store, &mut session, &mut entered, &mut prepared);
    Ok((session, entered, prepared))
}

/// What the Functions of the app compute for a scenario as it opens, in its language: what the
/// home page looks at to find the Functions that fail. `app` is the app as loaded once for
/// every scenario.
pub(super) fn run_scenario(
    state: &ServerState,
    app: &App,
    store: &Store,
    scenario: &lsf_core::store::CheckoutScenario,
) -> Result<CheckoutResult, String> {
    let (session, mut checkout, _) =
        scenario_start(state, store, scenario).map_err(|problems| problems.join("; "))?;
    let language = scenario
        .language
        .as_deref()
        .and_then(|iso| store.language(iso))
        .unwrap_or_else(|| store.primary_language());
    let selection = selection_for(app, &options(state).extensions, Some(scenario))?;
    Ok(price_with(
        app,
        &selection,
        &session,
        store,
        &mut checkout,
        language,
        BuyerJourneyStep::Interaction,
    ))
}

fn escape_query(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// The look of the checkout: `?variant=` when the request has it, which the session then keeps,
/// otherwise what the session kept, otherwise the server's.
fn variant_of(state: &ServerState, session_id: &str, incoming: &Incoming) -> Variant {
    let asked = incoming.query_param("variant").and_then(Variant::from_name);
    state.with_session(session_id, |entry| {
        if asked.is_some() {
            entry.checkout.variant = asked;
        }
        entry
            .checkout
            .variant
            .unwrap_or_else(|| options(state).variant)
    })
}

/// The scheme and host the visitor reached the server by.
fn origin_of(incoming: &Incoming) -> String {
    format!(
        "{}://{}",
        incoming.header("x-forwarded-proto").unwrap_or("http"),
        incoming.host
    )
}

/// The checkout as the page reads it: the result of the Functions and everything around it.
struct Computed {
    json: Json,
    result: CheckoutResult,
    /// What the buyer entered, with what the store knows of them.
    checkout: CheckoutState,
    /// What the session held when it was read.
    basis: Basis,
}

/// Everything the session holds that the checkout is computed from, read in one step, with the
/// basis that tells later whether it still holds.
fn read(state: &ServerState, session_id: &str) -> (Session, Arc<Store>, CheckoutState, Basis) {
    let loaded = state.loaded().store.clone();
    state.with_session(session_id, |entry| {
        // Caught up with a customer that changed since (a login link, the control API, the
        // login form of the theme), in the same step as the read.
        let store = entry.store.clone().unwrap_or_else(|| loaded.clone());
        checkout_account::prepare(
            &store,
            &mut entry.session,
            &mut entry.checkout.state,
            &mut entry.checkout.prepared,
        );
        let basis = Basis::of(entry, &loaded);
        (
            entry.session.clone(),
            basis.store.0.clone(),
            entry.checkout.state.clone(),
            basis,
        )
    })
}

/// Runs the Functions of the app on a cart: what the checkout looks like, with the app and the
/// extensions selected. A logged-in customer does not have to type what the store knows.
///
/// The Functions read the language of the buyer (`localization.language`) and the step of the
/// buyer journey (`buyerJourney.step`), so every run takes them: the only way to run the
/// Functions is through here, with the language and the step of the request.
fn price(
    state: &ServerState,
    session: &Session,
    store: &Store,
    checkout: &mut CheckoutState,
    language: &Language,
    step: BuyerJourneyStep,
) -> Result<(App, Selection, CheckoutResult), String> {
    let options = options(state);
    let app = lsf_checkout::app::load(&options.app_dir)?;
    // The scenario the checkout opened from sets it up, as long as it exists and is valid.
    let scenario = scenario_named(store, checkout.scenario.as_deref());
    let selection = selection_for(&app, &options.extensions, scenario)?;
    let result = price_with(&app, &selection, session, store, checkout, language, step);
    Ok((app, selection, result))
}

/// [`price`] with the app loaded and the extensions selected.
fn price_with(
    app: &App,
    selection: &Selection,
    session: &Session,
    store: &Store,
    checkout: &mut CheckoutState,
    language: &Language,
    step: BuyerJourneyStep,
) -> CheckoutResult {
    checkout.language = Some(language.iso_code.clone());
    checkout.step = step;
    let customer = session.customer_id.and_then(|id| store.customer_by_id(id));
    if checkout.email.is_none() {
        checkout.email = customer.map(|customer| customer.email.clone());
    }
    if checkout.phone.is_none() {
        checkout.phone = customer
            .map(|customer| customer.phone.clone())
            .filter(|phone| !phone.is_empty());
    }
    let config = config_for(store, checkout.scenario.as_deref());
    pipeline::compute(app, selection, store, session, checkout, &config)
}

/// The checkout as the Functions compute it for `step`: `Interaction` for everything the buyer
/// does before the order (the page, the state, an update), `Completion` for placing the order.
fn compute(
    state: &ServerState,
    session_id: &str,
    incoming: &Incoming,
    step: BuyerJourneyStep,
) -> Result<Computed, String> {
    let variant = variant_of(state, session_id, incoming);
    // One read, so that the basis is what the result is computed from.
    let (session, store, mut checkout, basis) = read(state, session_id);
    let customer = session.customer_id.and_then(|id| store.customer_by_id(id));
    let language = language_of(state, &store, incoming);
    let (app, selection, result) = price(state, &session, &store, &mut checkout, language, step)?;

    let mut json = serde_json::to_value(&result).map_err(|error| error.to_string())?;
    let object = json.as_object_mut().expect("a result is an object");
    object.insert("token".into(), json!(session.cart_token));
    object.insert("variant".into(), json!(variant.name()));
    object.insert("status".into(), json!("open"));
    // How many orders the session placed before this checkout: what tells it from the next one
    // of the same session (the token is the one of the cart), as scope of the extensions' storage.
    object.insert("orderGeneration".into(), json!(basis.placed));
    // The scenario the checkout was opened from, and where the list of them is.
    object.insert(
        "scenario".into(),
        json!(
            scenario_named(&store, checkout.scenario.as_deref())
                .map(|scenario| json!({"name": scenario.name, "title": scenario.title}))
        ),
    );
    object.insert("home".into(), json!(home_path(state, root_of(language))));
    object.insert(
        "shop".into(),
        json!({
            "name": store.shop.name,
            "currencyCode": store.shop.currency,
            "moneyFormat": store.shop.money_format,
            // The primary language of the store. The buyer's is `localization.language`.
            "locale": store.primary_language().iso_code,
            "storefrontUrl": origin_of(incoming),
            // The countries the store sells in, for the country selector.
            "countries": store.countries.iter()
                .map(|country| json!({"code": country.iso_code, "name": country.name}))
                .collect::<Vec<_>>(),
        }),
    );
    object.insert(
        "buyer".into(),
        json!({
            "email": checkout.email,
            "phone": checkout.phone,
            "customer": customer.map(customer_json),
            "purchasingCompany": customer.and_then(|customer| {
                purchasing_company_json(customer, &store, session.company_location)
            }),
        }),
    );
    // The customers of the store data to log in as, and what the logged-in one brings.
    object.insert(
        "account".into(),
        checkout_account::account_json(&store, &session),
    );
    object.insert(
        "localization".into(),
        localization_json(&store, language, &session, &checkout),
    );
    object.insert("shippingAddress".into(), json!(checkout.shipping_address));
    object.insert(
        "note".into(),
        json!((!session.cart_note.is_empty()).then_some(&session.cart_note)),
    );
    object.insert(
        "attributes".into(),
        Json::Array(
            session
                .cart_attributes
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        ),
    );
    object.insert("metafields".into(), json!(checkout.metafields));
    // A method a Function hid is not selected any more.
    let selected = checkout.selected_payment.as_ref().filter(|handle| {
        result
            .payment_methods
            .iter()
            .any(|method| &method.handle == *handle)
    });
    object.insert("selectedPayment".into(), json!(selected));
    object.insert("extensions".into(), extensions_json(&app, &selection));
    object.insert(
        "diagnostics".into(),
        Json::Array(
            app.diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic
                        .extension
                        .as_deref()
                        .is_none_or(|handle| selection.contains(handle))
                })
                .map(|diagnostic| json!(diagnostic.to_string()))
                .collect(),
        ),
    );
    Ok(Computed {
        json,
        result,
        checkout,
        basis,
    })
}

/// The customer as the Buyer Identity API reads it, with what the store knows of them.
fn customer_json(customer: &Customer) -> Json {
    json!({
        "id": format!("gid://shopify/Customer/{}", customer.id),
        "email": customer.email,
        "phone": (!customer.phone.is_empty()).then_some(&customer.phone),
        "firstName": customer.first_name,
        "lastName": customer.last_name,
        "acceptsMarketing": customer.accepts_marketing,
        "ordersCount": customer.orders.len(),
    })
}

/// The company and location a B2B customer buys for.
fn purchasing_company_json(
    customer: &Customer,
    store: &Store,
    selected_location: Option<u64>,
) -> Option<Json> {
    let company = &store.companies[customer.company?];
    let location = customer
        .current_location(selected_location, store)
        .map(|index| &company.locations[index]);
    Some(json!({
        "company": {
            "id": format!("gid://shopify/Company/{}", company.id),
            "name": company.name,
            "externalId": company.external_id,
        },
        "location": location.map(|location| json!({
            "id": format!("gid://shopify/CompanyLocation/{}", location.id),
            "name": location.name,
            "externalId": location.external_id,
        })),
    }))
}

fn language_json(language: &Language) -> Json {
    json!({
        "isoCode": language.iso_code,
        "name": language.name,
        "endonymName": language.endonym_name,
    })
}

/// What the Localization API reads: the language of the buyer (not the primary one of the
/// store), the country of the shipping address or else the one the visitor selected, the
/// currency and the market.
fn localization_json(
    store: &Store,
    language: &Language,
    session: &Session,
    checkout: &CheckoutState,
) -> Json {
    let country = checkout
        .shipping_address
        .as_ref()
        .and_then(|address| address.country_code.clone())
        .or_else(|| session.country.clone())
        .map(|code| code.to_ascii_uppercase());
    json!({
        "language": language_json(language),
        // Where the storefront of this language lives: `/fr`, nothing for the primary one.
        "rootUrl": root_of(language),
        "country": country.map(|code| json!({
            "isoCode": code,
            "name": store.country(&code).map_or(code.as_str(), |country| country.name.as_str()),
        })),
        "currency": {"isoCode": store.shop.currency},
        // The same market as `localization.market` of the theme.
        "market": {"id": "gid://shopify/Market/1", "handle": "primary"},
    })
}

/// The checkout UI extensions to load: the built ones on a supported API version.
fn extensions_json(app: &App, selection: &Selection) -> Json {
    Json::Array(
        selection
            .ui_extensions(app)
            .filter(|extension| extension.built() && extension.version.is_some())
            .map(|extension| {
                let file = extension
                    .bundle
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut targets: Vec<&str> = Vec::new();
                for target in &extension.targets {
                    if !targets.contains(&target.target.as_str()) {
                        targets.push(&target.target);
                    }
                }
                let translations = translations(&extension.dir);
                json!({
                    "handle": extension.handle,
                    "apiVersion": extension.api_version,
                    "script": format!("/__lsf/extensions/{}/{file}", extension.handle),
                    "targets": targets,
                    "capabilities": extension.capabilities,
                    "settings": setting_values(&extension.settings),
                    "metafields": extension.metafields.iter().map(|declared| {
                        let mut entry = json!({"namespace": declared.namespace, "key": declared.key});
                        if let Some(owner_type) = &declared.owner_type {
                            entry["ownerType"] = json!(owner_type);
                        }
                        entry
                    }).collect::<Vec<_>>(),
                    "translations": translations.strings,
                    "defaultLocale": translations.default_language,
                })
            })
            .collect(),
    )
}

/// The values the extension reads from `shopify.settings`. `[extensions.settings]` of the
/// manifest declares the fields (`fields`) a merchant fills in; nothing here fills them in, so
/// only the entries that are not that declaration are values.
fn setting_values(settings: &Json) -> Json {
    match settings {
        Json::Object(entries) => Json::Object(
            entries
                .iter()
                .filter(|(key, _)| key.as_str() != "fields")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        _ => json!({}),
    }
}

/// The state of the session's checkout: the order that was completed, if the cart was emptied by
/// completing it, otherwise the open checkout.
fn current(state: &ServerState, session_id: &str, incoming: &Incoming) -> Result<Json, Reply> {
    let completed = state.with_session(session_id, |entry| {
        let empty = entry.session.cart_lines.is_empty();
        entry.checkout.completed.clone().filter(|_| empty)
    });
    if let Some(completed) = completed {
        return Ok(in_request_language(state, session_id, incoming, completed));
    }
    // A cart again after an order: a new checkout.
    state.with_session(session_id, |entry| entry.checkout.completed = None);
    compute(state, session_id, incoming, BuyerJourneyStep::Interaction)
        .map(|computed| computed.json)
        .map_err(|problem| error(500, problem))
}

/// An order that was placed, read in the language of the request: the order is the one that was
/// placed, the language it is read in is not the one it was placed in.
fn in_request_language(
    state: &ServerState,
    session_id: &str,
    incoming: &Incoming,
    mut completed: Json,
) -> Json {
    let (_, store) = state.snapshot(session_id);
    let language = language_of(state, &store, incoming);
    completed["localization"]["language"] = language_json(language);
    completed["localization"]["rootUrl"] = json!(root_of(language));
    completed
}

fn state_json(state: &ServerState, incoming: &Incoming, token: &str) -> Reply {
    let (id, is_new) = match session_of(state, incoming, token) {
        Ok(session) => session,
        Err(reply) => return reply,
    };
    let reply = match current(state, &id, incoming) {
        Ok(json) => Reply::json(200, &json).cached(cache::PRIVATE),
        Err(reply) => reply,
    };
    with_cookie(reply, &id, is_new)
}

/// `GET /__lsf/checkout?session=<id>`: the state of a session's checkout, for tests.
fn control_state(state: &ServerState, incoming: &Incoming) -> Reply {
    let id = match incoming.query_param("session") {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => state.session_id(incoming).0,
    };
    match current(state, &id, incoming) {
        Ok(json) => Reply::json(200, &json).cached(cache::PRIVATE),
        Err(reply) => reply,
    }
}

/// The HTML of the page with the state in it.
fn shell(json: &Json) -> Reply {
    let template = lsf_checkout::assets::get("checkout.html").expect("checkout.html is embedded");
    let template = String::from_utf8_lossy(template);
    // Inside a script element: nothing in the JSON may close it or open a comment.
    let state = json
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    // The page is in the language of the buyer.
    let lang: String = json["localization"]["language"]["isoCode"]
        .as_str()
        .unwrap_or("en")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let html = template
        .replacen("<html lang=\"en\">", &format!("<html lang=\"{lang}\">"), 1)
        .replace("<!--LSF_ASSETS-->", ASSETS)
        .replace("<!--LSF_STATE-->", &state);
    Reply::html(200, html).cached(cache::PRIVATE)
}

fn page(state: &ServerState, incoming: &Incoming, token: &str) -> Reply {
    let (id, is_new) = match session_of(state, incoming, token) {
        Ok(session) => session,
        Err(reply) => return reply,
    };
    let reply = match current(state, &id, incoming) {
        Ok(json) => shell(&json),
        Err(reply) => reply,
    };
    with_cookie(reply, &id, is_new)
}

fn thank_you(state: &ServerState, incoming: &Incoming, token: &str) -> Reply {
    let (id, is_new) = match session_of(state, incoming, token) {
        Ok(session) => session,
        Err(reply) => return reply,
    };
    let completed = state.with_session(&id, |entry| entry.checkout.completed.clone());
    let reply = match completed {
        Some(json) => shell(&in_request_language(state, &id, incoming, json)),
        None => Reply::redirect(&checkout_path(&request_root(state, &id, incoming), token)),
    };
    with_cookie(reply, &id, is_new)
}

/// `POST …/update`: applies the operations in the body, one after the other, and answers with
/// the new state. When one fails none is applied.
fn update(state: &ServerState, incoming: &Incoming, token: &str) -> Reply {
    let (id, is_new) = match session_of(state, incoming, token) {
        Ok(session) => session,
        Err(reply) => return reply,
    };
    let Some(operations) = incoming.params.get("operations").and_then(Json::as_array) else {
        return error(400, "The body must be {\"operations\": [...]}.");
    };
    let reply = apply_update(state, incoming, &id, operations, || {});
    with_cookie(reply, &id, is_new)
}

/// Applies the operations of an update to the session they were read from. `between` runs after
/// they were applied and before they are written, where another request may change the session.
///
/// The write is one step of the session that checks the session is still what the operations
/// were applied to: an update never reopens an order that was placed, restores lines the order
/// took, or overwrites a change made meanwhile. In those cases nothing is written and the answer
/// is the checkout as it is now, with status 409.
pub(super) fn apply_update(
    state: &ServerState,
    incoming: &Incoming,
    id: &str,
    operations: &[Json],
    between: impl FnOnce(),
) -> Reply {
    // An order that was placed is final: the cart it took is empty, and there is nothing to change.
    if already_placed(state, id) {
        return conflict(state, incoming, id);
    }
    let (mut session, store, mut checkout, basis) = read(state, id);
    let config = config_for(&store, checkout.scenario.as_deref());
    // The operations that run the Functions (a change of a line) see the checkout the buyer sees.
    let language = language_of(state, &store, incoming);
    for (position, operation) in operations.iter().enumerate() {
        if let Err(problem) = apply(
            state,
            operation,
            &mut session,
            &mut checkout,
            &store,
            &config,
            language,
        ) {
            return error(400, format!("operation {position}: {problem}"));
        }
    }
    between();
    let identity = Identity::of(&store, &session);
    let written = state.with_session(id, |entry| {
        let unchanged = !is_placed(entry) && Basis::of(entry, &state.loaded().store) == basis;
        if unchanged {
            entry.session.cart_lines = session.cart_lines;
            entry.session.cart_note = session.cart_note;
            entry.session.cart_attributes = session.cart_attributes;
            entry.session.country = session.country;
            entry.session.customer_id = session.customer_id;
            entry.session.company_location = session.company_location;
            entry.checkout.prepared = Some(identity);
            entry.checkout.state = checkout;
            // Changing a cart after an order starts another checkout.
            entry.checkout.completed = None;
        }
        unchanged
    });
    if !written {
        return conflict(state, incoming, id);
    }
    match compute(state, id, incoming, BuyerJourneyStep::Interaction) {
        Ok(computed) => Reply::json(200, &computed.json).cached(cache::PRIVATE),
        Err(problem) => error(500, problem),
    }
}

/// The answer to a request that was made on a checkout that is not the one it saw any more:
/// nothing was done, and the buyer sees the checkout as it is now (the order, if it was placed).
fn conflict(state: &ServerState, incoming: &Incoming, id: &str) -> Reply {
    match current(state, id, incoming) {
        Ok(fresh) => Reply::json(409, &fresh).cached(cache::PRIVATE),
        Err(reply) => reply,
    }
}

fn text_of(operation: &Json, key: &str) -> Option<String> {
    operation
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn change_of<'a>(operation: &'a Json, kind: &str) -> Result<&'a Json, String> {
    operation
        .get("change")
        .filter(|change| change.is_object())
        .ok_or_else(|| format!("a `{kind}` operation needs a `change`"))
}

/// Applies one operation to the cart and the choices of the buyer.
fn apply(
    state: &ServerState,
    operation: &Json,
    session: &mut Session,
    checkout: &mut CheckoutState,
    store: &Store,
    config: &lsf_checkout::CheckoutConfig,
    language: &Language,
) -> Result<(), String> {
    let kind = operation
        .get("type")
        .and_then(Json::as_str)
        .ok_or("an operation needs a `type`")?;
    match kind {
        "customer" => checkout_account::log_in(store, session, checkout, operation)?,
        "companyLocation" => {
            checkout_account::choose_location(store, session, checkout, operation)?
        }
        "email" => checkout.email = text_of(operation, "value"),
        "phone" => checkout.phone = text_of(operation, "value"),
        "shippingAddress" => {
            let address = operation
                .get("address")
                .filter(|address| !address.is_null())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut address: Address = serde_json::from_value(address)
                .map_err(|problem| format!("the address is not valid: {problem}"))?;
            for field in [
                &mut address.first_name,
                &mut address.last_name,
                &mut address.company,
                &mut address.address1,
                &mut address.address2,
                &mut address.city,
                &mut address.country_code,
                &mut address.province_code,
                &mut address.zip,
                &mut address.phone,
            ] {
                if field.as_deref().is_some_and(|text| text.trim().is_empty()) {
                    *field = None;
                }
            }
            if let Some(code) = &mut address.country_code {
                *code = code.trim().to_ascii_uppercase();
                // Markets decide on the country of the address.
                if store.country(code).is_some() {
                    session.country = Some(code.clone());
                }
            }
            checkout.shipping_address = (address != Address::default()).then_some(address);
        }
        "deliveryOption" => {
            let group = text_of(operation, "group").unwrap_or_else(|| DELIVERY_GROUP_HANDLE.into());
            let handle =
                text_of(operation, "handle").ok_or("a `deliveryOption` needs a `handle`")?;
            if group != DELIVERY_GROUP_HANDLE {
                return Err(format!(
                    "there is no delivery group `{group}` (the group is `{DELIVERY_GROUP_HANDLE}`)"
                ));
            }
            if !config
                .shipping_rates
                .iter()
                .any(|rate| rate.handle == handle)
            {
                let known: Vec<&str> = config
                    .shipping_rates
                    .iter()
                    .map(|rate| rate.handle.as_str())
                    .collect();
                return Err(format!(
                    "there is no delivery option `{handle}` (the options are {})",
                    known.join(", ")
                ));
            }
            checkout.selected_delivery.insert(group, handle);
        }
        "paymentMethod" => match text_of(operation, "handle") {
            None => checkout.selected_payment = None,
            Some(handle) => {
                if !config
                    .payment_methods
                    .iter()
                    .any(|method| method.handle == handle)
                {
                    let known: Vec<&str> = config
                        .payment_methods
                        .iter()
                        .map(|method| method.handle.as_str())
                        .collect();
                    return Err(format!(
                        "there is no payment method `{handle}` (the methods are {})",
                        known.join(", ")
                    ));
                }
                checkout.selected_payment = Some(handle);
            }
        },
        "discountCode" => {
            let code = text_of(operation, "code").ok_or("a `discountCode` needs a `code`")?;
            match operation.get("action").and_then(Json::as_str) {
                Some("add") => {
                    if !checkout
                        .discount_codes
                        .iter()
                        .any(|entered| entered.eq_ignore_ascii_case(&code))
                    {
                        checkout.discount_codes.push(code);
                    }
                }
                Some("remove") => checkout
                    .discount_codes
                    .retain(|entered| !entered.eq_ignore_ascii_case(&code)),
                _ => return Err("the `action` of a `discountCode` is `add` or `remove`".into()),
            }
        }
        "cartLines" => cart_lines(
            state,
            change_of(operation, kind)?,
            session,
            checkout,
            store,
            language,
        )?,
        "attribute" => {
            let change = change_of(operation, kind)?;
            let key = text_of(change, "key").ok_or("an attribute change needs a `key`")?;
            match change.get("type").and_then(Json::as_str) {
                Some("updateAttribute") => {
                    let value = change
                        .get("value")
                        .and_then(Json::as_str)
                        .unwrap_or_default();
                    session.cart_attributes.insert(key, value.to_string());
                }
                Some("removeAttribute") => {
                    session.cart_attributes.shift_remove(&key);
                }
                other => return Err(format!("unknown attribute change {other:?}")),
            }
        }
        "note" => {
            let change = change_of(operation, kind)?;
            match change.get("type").and_then(Json::as_str) {
                Some("updateNote") => {
                    session.cart_note = change
                        .get("note")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_string();
                }
                Some("removeNote") => session.cart_note.clear(),
                other => return Err(format!("unknown note change {other:?}")),
            }
        }
        "metafield" => {
            let change = change_of(operation, kind)?;
            let namespace =
                text_of(change, "namespace").ok_or("a metafield change needs a `namespace`")?;
            let key = text_of(change, "key").ok_or("a metafield change needs a `key`")?;
            checkout
                .metafields
                .retain(|field| !(field.namespace == namespace && field.key == key));
            match change.get("type").and_then(Json::as_str) {
                Some("updateMetafield" | "updateCartMetafield") => {
                    checkout.metafields.push(Metafield {
                        namespace,
                        key,
                        kind: text_of(change, "valueType")
                            .unwrap_or_else(|| "single_line_text_field".into()),
                        value: change
                            .get("value")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                Some("removeMetafield" | "removeCartMetafield") => {}
                other => return Err(format!("unknown metafield change {other:?}")),
            }
        }
        other => return Err(format!("unknown operation `{other}`")),
    }
    Ok(())
}

/// The number at the end of a gid (`gid://shopify/ProductVariant/42`), or a plain number.
fn gid_number(id: &str) -> Option<u64> {
    id.rsplit('/').next()?.parse().ok()
}

/// The line of the checkout a change names by its id. The ids are those of the checkout, not
/// positions in the cart: after a cart transform a line may be a merge of several lines of the
/// cart.
fn checkout_line(
    state: &ServerState,
    change: &Json,
    session: &Session,
    checkout: &CheckoutState,
    store: &Store,
    language: &Language,
) -> Result<ResultLine, String> {
    let id = change
        .get("id")
        .and_then(Json::as_str)
        .ok_or("the change needs the `id` of a line")?;
    let mut checkout = checkout.clone();
    let (_, _, result) = price(
        state,
        session,
        store,
        &mut checkout,
        language,
        BuyerJourneyStep::Interaction,
    )?;
    result
        .lines
        .into_iter()
        .find(|line| line.id == id)
        .filter(|line| !line.sources.is_empty())
        .ok_or_else(|| format!("there is no line `{id}` in the cart"))
}

/// Changes the quantity of a checkout line to `quantity` by changing the lines of the cart it is
/// made of: each holds the same share of the new quantity as of the old one. A line the share of
/// which falls to nothing leaves the cart.
pub(super) fn requantify(
    session: &mut Session,
    line: &ResultLine,
    quantity: u32,
) -> Result<(), String> {
    let old = u64::from(line.quantity.max(1));
    for source in &line.sources {
        let held = u64::from(source.quantity);
        let share = if quantity == 0 {
            0
        } else {
            ((held * u64::from(quantity) + old / 2) / old).max(1)
        };
        let share = u32::try_from(share).map_err(|_| "the quantity is too large".to_string())?;
        let cart_line = &mut session.cart_lines[source.index];
        cart_line.quantity = (cart_line.quantity - source.quantity.min(cart_line.quantity))
            .checked_add(share)
            .ok_or("the quantity is too large")?;
    }
    session
        .cart_lines
        .retain(|cart_line| cart_line.quantity > 0);
    Ok(())
}

fn line_properties(change: &Json) -> Option<indexmap::IndexMap<String, String>> {
    change
        .get("attributes")
        .and_then(Json::as_array)
        .map(|attributes| {
            attributes
                .iter()
                .filter_map(|attribute| {
                    let key = attribute.get("key")?.as_str()?;
                    let value = attribute.get("value")?.as_str()?;
                    (!value.is_empty()).then(|| (key.to_string(), value.to_string()))
                })
                .collect()
        })
}

fn selling_plan(change: &Json, store: &Store, variant_id: u64) -> Result<Option<u64>, String> {
    let Some(id) = change.get("sellingPlanId").and_then(Json::as_str) else {
        return Ok(None);
    };
    let plan = gid_number(id).ok_or_else(|| format!("`{id}` is not a selling plan id"))?;
    let (product, _) = store.variant(variant_id).ok_or("unknown variant")?;
    store
        .selling_plan_of(product, plan)
        .map(|_| Some(plan))
        .ok_or_else(|| format!("the selling plan `{id}` is not offered for this product"))
}

fn quantity(change: &Json) -> Result<Option<u32>, String> {
    match change.get("quantity") {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|quantity| u32::try_from(quantity).ok())
            .map(Some)
            .ok_or_else(|| "the quantity must be a whole number".to_string()),
    }
}

/// Applies an `updateCartLine` to a line of the checkout, through the lines of the cart it is
/// made of. A cart line may be shared: after a partial merge, a cart line of three T-shirts
/// holds one in a bundle and two in a line of its own. What a change does to a line must stay
/// within the share of that line, so a change of what the line *is* (its merchandise, its
/// attributes, its selling plan) splits the cart line, and the other share is left as it was.
pub(super) fn update_line(
    session: &mut Session,
    line: &ResultLine,
    change: &Json,
    store: &Store,
) -> Result<(), String> {
    let new_quantity = quantity(change)?;
    if new_quantity == Some(0) {
        return requantify(session, line, 0);
    }
    let merchandise = change.get("merchandiseId").and_then(Json::as_str);
    if let (Some(kind), true) = (&line.transformed_by, merchandise.is_some()) {
        return Err(format!(
            "the line `{}` was changed by a cart transform ({kind}): its merchandise cannot be changed",
            line.id
        ));
    }
    let own_changes = merchandise.is_some()
        || change.get("attributes").is_some()
        || change.get("sellingPlanId").is_some();
    let [source] = line.sources.as_slice() else {
        if own_changes {
            return Err(format!(
                "the line `{}` is made of several lines by a cart transform: only its quantity can be changed",
                line.id
            ));
        }
        if let Some(quantity) = new_quantity {
            requantify(session, line, quantity)?;
        }
        return Ok(());
    };
    let index = source.index;
    // The cart line as the change would leave it; nothing is written before all is checked.
    let mut changed = session.cart_lines[index].clone();
    if let Some(merchandise) = merchandise {
        let variant_id = gid_number(merchandise)
            .ok_or_else(|| format!("`{merchandise}` is not a product variant id"))?;
        if store.variant(variant_id).is_none() {
            return Err(format!("there is no product variant `{merchandise}`"));
        }
        changed.variant_id = variant_id;
    }
    if let Some(properties) = line_properties(change) {
        changed.properties = properties;
    }
    if change.get("sellingPlanId").is_some() {
        changed.selling_plan = selling_plan(change, store, changed.variant_id)?;
    }
    let shared = session.cart_lines[index].quantity > source.quantity;
    if shared && line_key(&changed) != line_key(&session.cart_lines[index]) {
        // The share of this line moves to a line of its own, at the end of the cart so that the
        // position of the other lines (which a cart transform may name) does not change.
        session.cart_lines[index].quantity -= source.quantity;
        changed.quantity = new_quantity.unwrap_or(source.quantity);
        session.cart_lines.push(changed);
        return Ok(());
    }
    session.cart_lines[index] = changed;
    if let Some(quantity) = new_quantity {
        requantify(session, line, quantity)?;
    }
    Ok(())
}

/// `applyCartLinesChange` of the UI extension API: `addCartLine`, `removeCartLine` and
/// `updateCartLine`. The lines it names are those of the checkout; a line a cart transform made
/// from several lines of the cart is removed or scaled through them.
fn cart_lines(
    state: &ServerState,
    change: &Json,
    session: &mut Session,
    checkout: &CheckoutState,
    store: &Store,
    language: &Language,
) -> Result<(), String> {
    match change.get("type").and_then(Json::as_str) {
        Some("addCartLine") => {
            let merchandise = change
                .get("merchandiseId")
                .and_then(Json::as_str)
                .ok_or("addCartLine needs a `merchandiseId`")?;
            let variant_id = gid_number(merchandise)
                .ok_or_else(|| format!("`{merchandise}` is not a product variant id"))?;
            let (_, variant) = store
                .variant(variant_id)
                .ok_or_else(|| format!("there is no product variant `{merchandise}`"))?;
            if !variant.available {
                return Err(format!("the product variant `{merchandise}` is sold out"));
            }
            let quantity = quantity(change)?
                .filter(|quantity| *quantity > 0)
                .unwrap_or(1);
            let line = CartLine {
                variant_id,
                quantity,
                properties: line_properties(change).unwrap_or_default(),
                selling_plan: selling_plan(change, store, variant_id)?,
            };
            let key = line_key(&line);
            match session
                .cart_lines
                .iter_mut()
                .find(|existing| line_key(existing) == key)
            {
                Some(existing) => existing.quantity += quantity,
                None => session.cart_lines.push(line),
            }
        }
        Some("removeCartLine") => {
            let line = checkout_line(state, change, session, checkout, store, language)?;
            let removed = quantity(change)?.ok_or("removeCartLine needs a `quantity`")?;
            requantify(session, &line, line.quantity.saturating_sub(removed))?;
        }
        Some("updateCartLine") => {
            let line = checkout_line(state, change, session, checkout, store, language)?;
            update_line(session, &line, change, store)?;
        }
        other => return Err(format!("unknown cart line change {other:?}")),
    }
    Ok(())
}

/// What the buyer has to fill before paying, as errors on the fields of the page.
fn missing(computed: &Computed, state: &CheckoutState) -> Vec<Json> {
    let error = |message: &str, target: &str| json!({"message": message, "target": target, "function": null});
    let mut errors = Vec::new();
    // The buyer the page shows: the email typed, or the one of the logged-in customer.
    let email = computed.json["buyer"]["email"].as_str();
    if email.is_none_or(str::is_empty) {
        errors.push(error("Enter an email", "$.cart.buyerIdentity.email"));
    }
    if computed
        .result
        .lines
        .iter()
        .any(|line| line.requires_shipping)
    {
        let groups = &computed.result.delivery_groups;
        if groups.is_empty() || groups.iter().any(|group| group.options.is_empty()) {
            errors.push(error(
                "No delivery option is available",
                "$.cart.deliveryGroups[0].selectedDeliveryOption",
            ));
        }
        let address = state.shipping_address.clone().unwrap_or_default();
        let field = |value: &Option<String>| value.as_deref().is_none_or(str::is_empty);
        let target = |name: &str| format!("$.cart.deliveryGroups[0].deliveryAddress.{name}");
        for (empty, message, name) in [
            (field(&address.last_name), "Enter a last name", "lastName"),
            (field(&address.address1), "Enter an address", "address1"),
            (field(&address.city), "Enter a city", "city"),
            (
                field(&address.country_code),
                "Select a country/region",
                "countryCode",
            ),
        ] {
            if empty {
                errors.push(error(message, &target(name)));
            }
        }
    }
    if computed.result.payment_methods.is_empty() {
        errors.push(error(
            "No payment method is available",
            "$.cart.selectedPayment",
        ));
    } else if computed.json["selectedPayment"].is_null() {
        errors.push(error("Select a payment method", "$.cart.selectedPayment"));
    }
    errors
}

/// `POST …/complete`: places the order when nothing blocks it.
fn complete(state: &ServerState, incoming: &Incoming, token: &str) -> Reply {
    let (id, is_new) = match session_of(state, incoming, token) {
        Ok(session) => session,
        Err(reply) => return reply,
    };
    let reply = place_order(state, incoming, &id, token, || {});
    with_cookie(reply, &id, is_new)
}

/// The way to the thank-you page, in the language of the request.
fn redirect_to_thanks(state: &ServerState, incoming: &Incoming, id: &str, token: &str) -> Reply {
    let root = request_root(state, id, incoming);
    Reply::json(
        200,
        &json!({"redirect": format!("{}/thank-you", checkout_path(&root, token))}),
    )
    .cached(cache::PRIVATE)
}

/// Whether the order of this checkout was placed already, and its cart taken: asking again
/// leads to the same thank-you page instead of placing another order.
fn already_placed(state: &ServerState, id: &str) -> bool {
    state.with_session(id, |entry| is_placed(entry))
}

fn is_placed(entry: &SessionEntry) -> bool {
    entry.checkout.completed.is_some() && entry.session.cart_lines.is_empty()
}

/// Places the order of the cart the Functions were run on. `between` runs after the session was
/// read and before the order takes the cart, where the Functions take their time.
pub(super) fn place_order(
    state: &ServerState,
    incoming: &Incoming,
    id: &str,
    token: &str,
    between: impl FnOnce(),
) -> Reply {
    if already_placed(state, id) {
        return redirect_to_thanks(state, incoming, id, token);
    }
    let mut computed = match compute(state, id, incoming, BuyerJourneyStep::Completion) {
        Ok(computed) => computed,
        Err(problem) => return error(500, problem),
    };
    between();
    if computed.result.lines.is_empty() {
        return error(422, "The cart is empty.");
    }
    let mut blocking = missing(&computed, &computed.checkout);
    if !computed.result.errors.is_empty() || !blocking.is_empty() {
        // What the Functions said stays first.
        if let Some(errors) = computed.json["errors"].as_array_mut() {
            errors.append(&mut blocking);
        }
        return Reply::json(422, &computed.json).cached(cache::PRIVATE);
    }
    let mut order = computed.json;
    // The cart is taken only if it is still the one that was priced; the check and the take are
    // one step of the session.
    let taken = state.with_session(id, |entry| {
        if Basis::of(entry, &state.loaded().store) != computed.basis {
            return false;
        }
        let number = 1000 + state.next_order();
        order["status"] = json!("completed");
        // The order costs the whole price: what is paid now is received, and what a selling
        // plan defers is outstanding (`totalReceivedSet` and `totalOutstandingSet` of Shopify's
        // Order).
        let (received, outstanding) = (&order["cost"]["dueNow"], &order["cost"]["dueLater"]);
        order["order"] = json!({
            "id": format!("gid://shopify/Order/{number}"),
            "number": format!("#{number}"),
            "totalReceived": received,
            "totalOutstanding": outstanding,
        });
        entry.checkout.completed = Some(order);
        entry.checkout.placed += 1;
        // The next checkout of the session is still the one of the scenario.
        entry.checkout.state = CheckoutState {
            scenario: entry.checkout.state.scenario.clone(),
            ..CheckoutState::default()
        };
        entry.session.cart_lines.clear();
        entry.session.cart_note.clear();
        entry.session.cart_attributes.clear();
        true
    });
    if taken {
        return redirect_to_thanks(state, incoming, id, token);
    }
    // Another request placed the order meanwhile.
    if already_placed(state, id) {
        return redirect_to_thanks(state, incoming, id, token);
    }
    // The cart changed while the order was being priced: nothing was bought, and the buyer
    // sees the cart as it is now.
    conflict(state, incoming, id)
}

/// A file of the page, from the binary.
fn asset(file: &str) -> Reply {
    match lsf_checkout::assets::get(file) {
        Some(content) => Reply::new(200, content_type(file), content).cached(cache::RENDERED),
        None => Reply::not_found(),
    }
}

/// A file of the build of a checkout UI extension: `<extension>/dist/<file>`.
fn extension_file(state: &ServerState, handle: &str, file: &[&str]) -> Reply {
    let options = options(state);
    let Ok(app) = lsf_checkout::app::load(&options.app_dir) else {
        return Reply::not_found();
    };
    let Ok(selection) = Selection::resolve(&app, &options.extensions) else {
        return Reply::not_found();
    };
    let Some(extension) = app
        .ui_extension(handle)
        .filter(|extension| selection.contains(&extension.handle))
    else {
        return Reply::not_found();
    };
    if file
        .iter()
        .any(|part| part.is_empty() || *part == ".." || *part == "." || part.contains(['/', '\\']))
    {
        return Reply::not_found();
    }
    let Some(directory) = extension.bundle.parent() else {
        return Reply::not_found();
    };
    // Only what the build wrote: a link in `dist/` must not lead out of it.
    let Ok(directory) = directory.canonicalize() else {
        return Reply::not_found();
    };
    let path = file
        .iter()
        .fold(directory.clone(), |path, part| path.join(part));
    let Ok(path) = path.canonicalize() else {
        return Reply::not_found();
    };
    if !path.starts_with(&directory) || !path.is_file() {
        return Reply::not_found();
    }
    let Some(kind) = extension_content_type(&path) else {
        return Reply::not_found();
    };
    match std::fs::read(&path) {
        // The page of an extension is a sandboxed frame, but nothing stops the extension from
        // sending it to one of its own files: with this policy such a file runs without the
        // origin of the checkout, which it would otherwise share.
        Ok(content) => Reply::new(200, kind, content)
            .header("content-security-policy", "sandbox allow-scripts")
            .header("x-content-type-options", "nosniff")
            .cached(cache::PRIVATE),
        Err(_) => Reply::not_found(),
    }
}

/// The content type of a file of an extension, when it is one a checkout UI extension needs:
/// scripts, styles, images and JSON. A page (HTML, XML) is never served.
fn extension_content_type(path: &std::path::Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "js" | "mjs"
            | "css"
            | "json"
            | "map"
            | "svg"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "ico"
    )
    .then(|| content_type(&format!("file.{extension}")))
}
