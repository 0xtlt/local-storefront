//! `lsf app webhook trigger`: one webhook, built by lsf, signed and sent, as
//! `shopify app webhook trigger` sends the sample of a topic, but without Shopify: the body is
//! built from the records of the store data (a product, an order, a customer of the shop the
//! server seeds) rather than asked of Shopify's servers, and the answer is printed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use chrono::Utc;
use lsf_admin::api::samples::ShopSamples;
use lsf_admin::state::ShopState;
use lsf_admin::webhooks::samples::Samples;
use lsf_admin::webhooks::subscriptions::is_local_http;
use lsf_admin::webhooks::{
    API_VERSION, Delivery, DeliveryFilter, DeliveryOptions, DeliveryState, Trigger, Webhooks,
    topics,
};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Subcommand,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    /// Send one webhook of a topic, as `shopify app webhook trigger` does: its payload, built by
    /// lsf, signed with the client secret, to `--address` or to the subscriptions of
    /// shopify.app.toml for the topic. Prints what was sent and the answer; exits with 1 when a
    /// delivery failed.
    Trigger(TriggerArgs),
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum DeliveryMethod {
    /// HTTP(S): `https://…`, or `http://` to this machine.
    Http,
    /// Google Cloud Pub/Sub: `pubsub://{project-id}:{topic-id}`. Not delivered by lsf.
    GooglePubSub,
    /// Amazon EventBridge: `arn:aws:events:…`. Not delivered by lsf.
    EventBridge,
}

#[derive(clap::Args)]
pub struct TriggerArgs {
    /// The webhook topic, as Shopify writes it (`orders/create`) or as the GraphQL enum does
    /// (`ORDERS_CREATE`).
    #[arg(long, value_name = "TOPIC")]
    topic: String,

    /// The API version of the payload. lsf builds those of 2026-10 only.
    #[arg(long, value_name = "VERSION", default_value = API_VERSION)]
    api_version: String,

    /// How the payload is delivered. Without it, what the address says.
    #[arg(long, value_enum, value_name = "METHOD")]
    delivery_method: Option<DeliveryMethod>,

    /// Where to send it: `https://…`, or `http://localhost:{port}/{path}`. Without it, to the
    /// `uri` of each subscription of shopify.app.toml to the topic.
    #[arg(long, value_name = "URL")]
    address: Option<String>,

    /// The secret the `X-Shopify-Hmac-Sha256` header is computed with. Default:
    /// `SHOPIFY_API_SECRET` of the environment, else of the app's `.env`, else lsf's local
    /// secret.
    #[arg(
        long,
        env = "LSF_CLIENT_SECRET",
        value_name = "SECRET",
        hide_env_values = true
    )]
    client_secret: Option<String>,

    /// The directory of a Shopify app (the one with shopify.app.toml). Defaults to the app the
    /// current folder is in. Shopify's CLI calls it `--path`.
    #[arg(long, alias = "path", env = "LSF_APP", value_name = "DIR")]
    app: Option<PathBuf>,

    /// Where the backend of the app listens: relative `uri`s are joined to it, and absolute ones
    /// on the origin of `application_url` are moved to it. Without it, `application_url`.
    #[arg(long, env = "LSF_APP_URL", value_name = "URL", value_parser = super::app_url)]
    app_url: Option<String>,

    /// Print the deliveries as JSON: their request, their attempts, their state.
    #[arg(long, short = 'j')]
    json: bool,
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    match args.command {
        Subcommand::Trigger(args) => trigger(theme, data, previews, args),
    }
}

/// The records a triggered webhook is built from: the shop the server seeds from the store data
/// (`--data`, else the `shopify-local` of the theme or of the app, else the demo store) and the
/// app, as the Admin API has it when the server starts. A topic whose type the store data has no
/// record of is built from lsf's sample of it, modelled on Shopify's example payload.
fn sample_records(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    app_dir: Option<&Path>,
) -> Result<ShopSamples, String> {
    let data = crate::app::data_with_app(theme, data, app_dir);
    let local = crate::app::App::open_without_theme(theme, data.as_deref(), previews)?;
    let (store, diagnostics) = local.load_store();
    if diagnostics.has_errors() {
        crate::output::print_diagnostics(&diagnostics);
        return Err("the store data has errors (see above)".to_string());
    }
    let app = app_dir.and_then(|dir| lsf_checkout::app::load(dir).ok());
    Ok(ShopSamples(ShopState::seeded(&store, app.as_ref())))
}

fn trigger(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: TriggerArgs,
) -> Result<ExitCode, String> {
    let topic = topics::written_form(&args.topic).ok_or_else(|| {
        let hint = topics::suggestion(&args.topic)
            .map(|topic| format!(" (did you mean `{topic}`?)"))
            .unwrap_or_default();
        format!(
            "Topic '{}' does not exist for ApiVersion '{API_VERSION}'{hint}",
            args.topic
        )
    })?;
    if args.api_version != API_VERSION {
        return Err(format!(
            "Api Version '{}' is not one lsf builds webhooks for: lsf serves {API_VERSION} only",
            args.api_version
        ));
    }
    if let Some(address) = &args.address {
        check_address(address, args.delivery_method)?;
    }
    // The app gives the subscriptions and the client secret; with an address, it may be left out.
    let app_dir = match (&args.app, &args.address) {
        (Some(app), _) => Some(app.clone()),
        (None, address) => match crate::checkout::app_of(None, "lsf app webhook trigger") {
            Ok(dir) => Some(dir),
            Err(_) if address.is_some() => None,
            Err(error) => return Err(format!("{error}; or give `--address URL`")),
        },
    };
    let config = match &app_dir {
        Some(dir) => lsf_checkout::app::load(dir)?.config,
        None => Default::default(),
    };
    let (secret, _) =
        lsf_admin::auth::client_secret(args.client_secret.as_deref(), app_dir.as_deref());
    let samples = sample_records(theme, data, previews, app_dir.as_deref())?;
    let Some(payload) = samples
        .sample(topic)
        .and_then(|sample| sample.payload(topic))
    else {
        return Err(format!(
            "lsf does not build the payload of `{topic}`: it builds {}",
            lsf_admin::webhooks::payload::BUILT_TOPICS.join(", ")
        ));
    };
    // As Shopify's: a triggered webhook is not retried.
    let options = DeliveryOptions {
        retry_delays: Vec::new(),
        ..DeliveryOptions::default()
    };
    let webhooks = Webhooks::for_app(&config, args.app_url.clone(), secret, options);
    let shop = samples.shop();
    let ids = webhooks.trigger(&Trigger {
        topic,
        shop: &shop,
        payload: &payload,
        address: args.address.as_deref(),
        at: Utc::now(),
    });
    if ids.is_empty() {
        return Err(format!(
            "no subscription to `{topic}` in {}: give `--address URL`, or subscribe to it in \
             [[webhooks.subscriptions]]",
            config_name(app_dir.as_deref())
        ));
    }
    let filter = DeliveryFilter {
        ids: Some(ids.clone()),
        ..Default::default()
    };
    let waited = webhooks.deliveries().wait(
        &filter,
        ids.len(),
        webhooks.deliveries().options().timeout + Duration::from_secs(5),
    );
    let deliveries = waited.deliveries;
    if args.json {
        let json = serde_json::json!({
            "ok": deliveries.iter().all(|delivery| delivery.state == DeliveryState::Delivered),
            "deliveries": deliveries,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
        );
    } else {
        for delivery in &deliveries {
            print_delivery(delivery);
        }
    }
    let delivered = deliveries
        .iter()
        .all(|delivery| delivery.state == DeliveryState::Delivered);
    Ok(if delivered {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// What `shopify app webhook trigger` checks of an address: its kind matches the delivery method,
/// and plain HTTP is for this machine only. lsf delivers over HTTP only.
fn check_address(address: &str, method: Option<DeliveryMethod>) -> Result<(), String> {
    let kind = if address.starts_with("pubsub:") {
        DeliveryMethod::GooglePubSub
    } else if address.starts_with("arn:aws:events:") {
        DeliveryMethod::EventBridge
    } else if address.to_ascii_lowercase().starts_with("https://") || is_local_http(address) {
        DeliveryMethod::Http
    } else {
        return Err(format!(
            "Can't deliver your webhook payload to `{address}`: for remote HTTP testing, use a URL \
             that starts with https://; for local HTTP testing, use http://localhost:{{port}}/{{url-path}}"
        ));
    };
    if let Some(method) = method
        && method != kind
    {
        return Err(format!(
            "Can't deliver your webhook payload to `{address}` using '{}'",
            method_name(method)
        ));
    }
    match kind {
        DeliveryMethod::Http => Ok(()),
        other => Err(format!(
            "lsf delivers webhooks over HTTP only, not with '{}': give an http(s) `--address`",
            method_name(other)
        )),
    }
}

fn method_name(method: DeliveryMethod) -> &'static str {
    match method {
        DeliveryMethod::Http => "http",
        DeliveryMethod::GooglePubSub => "google-pub-sub",
        DeliveryMethod::EventBridge => "event-bridge",
    }
}

fn config_name(app: Option<&Path>) -> String {
    app.map_or_else(
        || "shopify.app.toml".to_string(),
        |dir| dir.join("shopify.app.toml").display().to_string(),
    )
}

/// One delivery, as sent and as answered.
fn print_delivery(delivery: &Delivery) {
    let target = delivery.url.as_deref().unwrap_or("(nowhere)");
    let subscription = delivery
        .subscription
        .as_ref()
        .map(|subscription| format!(" ({})", subscription.id))
        .unwrap_or_default();
    println!("{} → {target}{subscription}", delivery.topic);
    if let Some(request) = &delivery.request {
        println!("POST {target}");
        for (name, value) in &request.headers {
            println!("{name}: {value}");
        }
        println!();
        // The body as sent, made readable: the signature is of the compact text.
        let body = serde_json::from_str::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|json| serde_json::to_string_pretty(&json).ok())
            .unwrap_or_else(|| request.body.clone());
        println!("{body}");
        println!();
    }
    for attempt in &delivery.attempts {
        match (attempt.status, &attempt.response) {
            (Some(status), response) => {
                let response = response.as_deref().unwrap_or("").trim();
                let shown: String = response.chars().take(200).collect();
                let more = if response.chars().count() > 200 {
                    "…"
                } else {
                    ""
                };
                println!("← {status} in {} ms {shown}{more}", attempt.duration_ms);
            }
            (None, _) => println!(
                "← no answer after {} ms: {}",
                attempt.duration_ms,
                attempt.error.as_deref().unwrap_or("")
            ),
        }
    }
    for note in &delivery.notes {
        println!("note: {note}");
    }
    match delivery.state {
        DeliveryState::Delivered => println!("delivered"),
        DeliveryState::Failed => println!(
            "failed: {}",
            delivery.reason.as_deref().unwrap_or("the delivery failed")
        ),
        DeliveryState::NotSent => println!(
            "not sent: {}",
            delivery.reason.as_deref().unwrap_or("lsf cannot send it")
        ),
        DeliveryState::Pending => println!("no answer yet: still pending when lsf stopped waiting"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_checked_as_shopifys_cli_checks_it() {
        assert!(check_address("https://hooks.example.com/x", None).is_ok());
        assert!(
            check_address("http://localhost:3000/webhooks", Some(DeliveryMethod::Http)).is_ok()
        );
        assert!(check_address("http://127.0.0.1:3000/webhooks", None).is_ok());
        let remote = check_address("http://hooks.example.com/x", None).unwrap_err();
        assert!(remote.contains("https://"), "{remote}");
        let pubsub = check_address("pubsub://project:topic", None).unwrap_err();
        assert!(pubsub.contains("google-pub-sub"), "{pubsub}");
        let mismatch = check_address(
            "https://hooks.example.com/x",
            Some(DeliveryMethod::EventBridge),
        )
        .unwrap_err();
        assert!(mismatch.contains("using 'event-bridge'"), "{mismatch}");
    }
}
