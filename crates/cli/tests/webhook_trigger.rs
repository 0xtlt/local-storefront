//! `lsf app webhook trigger`, against a backend of the test's own.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

/// What the backend received: the path, the headers (lowercase names), the body.
type Received = Arc<Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>;

/// A backend on a free port that answers every request with `status`.
fn backend(status: u16) -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let received: Received = Arc::default();
    let log = Arc::clone(&received);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            let (head_end, length) = loop {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break (None, 0);
                }
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    break (Some(end), length);
                }
            };
            let Some(end) = head_end else { continue };
            while bytes.len() < end + 4 + length {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
            let mut lines = head.split("\r\n");
            let path = lines
                .next()
                .and_then(|line| line.split(' ').nth(1))
                .unwrap_or("")
                .to_string();
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
                .collect();
            log.lock()
                .unwrap()
                .push((path, headers, bytes[end + 4..].to_vec()));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Answer\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
            );
        }
    });
    (url, received)
}

/// An app of the test's own, with `shopify.app.toml`.
struct App(PathBuf);

impl App {
    fn new(name: &str, toml: &str) -> App {
        let path =
            std::env::temp_dir().join(format!("lsf-webhook-trigger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("shopify.app.toml"), toml).unwrap();
        App(path)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lsf(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args(args)
        .current_dir(directory)
        .env_remove("LSF_APP")
        .env_remove("LSF_APP_URL")
        .env_remove("LSF_CLIENT_SECRET")
        .env_remove("SHOPIFY_API_SECRET")
        .env_remove("LSF_COMMAND")
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn a_webhook_is_sent_to_an_address_signed_with_the_client_secret() {
    let (url, received) = backend(200);
    let address = format!("{url}/webhooks/orders");
    let output = lsf(
        &std::env::temp_dir(),
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "ORDERS_CREATE",
            "--address",
            &address,
            "--client-secret",
            "hush",
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    let printed = text(&output);
    assert!(
        printed.contains("X-Shopify-Topic: orders/create"),
        "{printed}"
    );
    assert!(printed.contains("← 200 in"), "{printed}");
    assert!(printed.trim_end().ends_with("delivered"), "{printed}");

    let received = received.lock().unwrap();
    assert_eq!(received.len(), 1);
    let (path, headers, body) = &received[0];
    assert_eq!(path, "/webhooks/orders");
    assert_eq!(header(headers, "x-shopify-topic"), Some("orders/create"));
    // The shop and the records of the store data: the demo store, without one.
    assert_eq!(
        header(headers, "x-shopify-shop-domain"),
        Some("local-supply-co.myshopify.com")
    );
    assert_eq!(header(headers, "x-shopify-api-version"), Some("2026-10"));
    // The signature is the HMAC of the body with the secret given.
    let signature = header(headers, "x-shopify-hmac-sha256").unwrap();
    assert!(lsf_admin::auth::signing::verify_webhook_hmac(
        "hush", body, signature
    ));
    let payload: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert!(
        payload["name"].as_str().unwrap().starts_with('#'),
        "{payload}"
    );
    assert!(
        payload["admin_graphql_api_id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/Order/")
    );
    assert!(!payload["line_items"].as_array().unwrap().is_empty());
}

#[test]
fn without_an_address_it_goes_to_the_subscriptions_of_the_app() {
    let (url, received) = backend(200);
    let app = App::new(
        "subscriptions",
        r#"
client_id = "abc"
application_url = "https://tunnel.trycloudflare.com"

[webhooks]
api_version = "2026-10"

  [[webhooks.subscriptions]]
  topics = ["products/update"]
  uri = "/webhooks/products"
  include_fields = ["id", "title"]

  [[webhooks.subscriptions]]
  topics = ["products/update"]
  uri = "https://tunnel.trycloudflare.com/webhooks/all"
"#,
    );
    std::fs::write(app.0.join(".env"), "SHOPIFY_API_SECRET=from-dotenv\n").unwrap();
    let output = lsf(
        &app.0,
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "products/update",
            "--app-url",
            &url,
            "--json",
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["deliveries"].as_array().unwrap().len(), 2);

    let mut received = received.lock().unwrap().clone();
    received.sort_by(|a, b| a.0.cmp(&b.0));
    let paths: Vec<&str> = received.iter().map(|(path, ..)| path.as_str()).collect();
    assert_eq!(paths, ["/webhooks/all", "/webhooks/products"]);
    let (_, headers, body) = &received[1];
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(body).unwrap(),
        serde_json::json!({"id": 6551952040971u64, "title": "Organic Cotton T-Shirt"})
    );
    // The secret of the app's `.env`.
    let signature = header(headers, "x-shopify-hmac-sha256").unwrap();
    assert!(lsf_admin::auth::signing::verify_webhook_hmac(
        "from-dotenv",
        body,
        signature
    ));
}

#[test]
fn format_json_prints_the_deliveries_and_json_is_its_old_name() {
    let (url, _) = backend(200);
    let address = format!("{url}/webhooks");
    let trigger = |format: &[&str]| {
        let output = lsf(
            &std::env::temp_dir(),
            &[
                &[
                    "app",
                    "webhook",
                    "trigger",
                    "--topic",
                    "app/uninstalled",
                    "--address",
                    &address,
                ][..],
                format,
            ]
            .concat(),
        );
        assert!(output.status.success(), "{format:?}: {}", text(&output));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| panic!("{format:?}: {}", text(&output)));
        assert_eq!(json["ok"], true, "{json}");
        assert_eq!(json["deliveries"][0]["state"], "delivered", "{json}");
        assert_eq!(json["deliveries"][0]["topic"], "app/uninstalled", "{json}");
    };
    trigger(&["--format", "json"]);
    trigger(&["--json"]);
    trigger(&["-j"]);
    // Hidden: `--help` names `--format` only.
    let help = lsf(
        &std::env::temp_dir(),
        &["app", "webhook", "trigger", "--help"],
    );
    let help = text(&help);
    assert!(help.contains("--format <FORMAT>"), "{help}");
    assert!(!help.contains("--json"), "{help}");
}

#[test]
fn a_failed_delivery_exits_with_1() {
    let (url, _) = backend(500);
    let output = lsf(
        &std::env::temp_dir(),
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "app/uninstalled",
            "--address",
            &url,
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("failed: answered 500"),
        "{}",
        text(&output)
    );
}

#[test]
fn installed_with_npm_a_refused_delivery_names_the_command_to_give_the_secret_to() {
    let (url, _) = backend(401);
    let output = Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args([
            "app",
            "webhook",
            "trigger",
            "--topic",
            "app/uninstalled",
            "--address",
            &url,
        ])
        .current_dir(std::env::temp_dir())
        .env_remove("LSF_APP")
        .env_remove("LSF_APP_URL")
        .env_remove("LSF_CLIENT_SECRET")
        .env_remove("SHOPIFY_API_SECRET")
        .env("LSF_COMMAND", "npx local-storefront")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains(
            "its own: give npx local-storefront the backend's secret (--client-secret, or --app \
             for the SHOPIFY_API_SECRET of its .env)"
        ),
        "{}",
        text(&output)
    );
}

#[test]
fn a_refused_delivery_says_which_secret_lsf_signed_it_with() {
    let (url, _) = backend(401);
    let trigger = |secret: &[&str]| {
        let mut args = vec![
            "app",
            "webhook",
            "trigger",
            "--topic",
            "app/uninstalled",
            "--address",
            &url,
        ];
        args.extend(secret);
        lsf(&std::env::temp_dir(), &args)
    };
    // Without the app, its `.env` is not read: lsf's own secret, and how to give the backend's.
    let output = trigger(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains(
            "failed: answered 401 (1 attempt): lsf signed it with lsf-local-client-secret, its \
             own: give lsf the backend's secret (--client-secret, or --app for the \
             SHOPIFY_API_SECRET of its .env)"
        ),
        "{}",
        text(&output)
    );
    let output = trigger(&["--client-secret", "s3cr3t"]);
    assert!(
        text(&output).contains(
            "failed: answered 401 (1 attempt): lsf signed it with the client secret from \
             --client-secret: does the backend verify with the same one?"
        ),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("s3cr3t"), "{}", text(&output));
}

#[test]
fn what_cannot_be_sent_is_refused_with_why() {
    let dir = std::env::temp_dir();
    let refuse = |args: &[&str], said: &str| {
        let output = lsf(&dir, args);
        assert_eq!(output.status.code(), Some(2), "{}", text(&output));
        assert!(text(&output).contains(said), "{}", text(&output));
    };
    let base = [
        "app",
        "webhook",
        "trigger",
        "--address",
        "http://localhost:1/x",
    ];
    refuse(
        &[&base[..], &["--topic", "orders/created"]].concat(),
        "did you mean `orders/create`?",
    );
    refuse(
        &[
            &base[..],
            &["--topic", "orders/create", "--api-version", "2026-07"],
        ]
        .concat(),
        "is not one lsf builds webhooks for: lsf knows 2026-10",
    );
    refuse(
        &[&base[..], &["--topic", "shipping_addresses/create"]].concat(),
        "does not build the payload of `shipping_addresses/create`",
    );
    refuse(
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "orders/create",
            "--address",
            "pubsub://project:topic",
        ],
        "over HTTP only",
    );
    // No address, and no subscription to the topic in the app.
    let app = App::new("none", "client_id = \"abc\"\n");
    let output = lsf(
        &app.0,
        &["app", "webhook", "trigger", "--topic", "orders/create"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains("no subscription to `orders/create`"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_subscription_that_leaves_the_machine_is_refused_before_anything_is_sent() {
    let (url, received) = backend(200);
    let app = App::new(
        "remote",
        r#"
client_id = "abc"
application_url = "https://my-app.example.com"

[webhooks]
api_version = "2026-10"

  [[webhooks.subscriptions]]
  topics = ["products/update"]
  uri = "/webhooks/products"

  [[webhooks.subscriptions]]
  topics = ["orders/create"]
  uri = "https://hooks.example.net/orders"
"#,
    );
    // `trigger` starts no server: it says what to give it.
    let advice = "is neither; give --app-url http://localhost:<port> (your backend) or \
                  --address <url>";
    // A relative `uri`, and no `--app-url`: it would join `application_url`.
    let output = lsf(
        &app.0,
        &["app", "webhook", "trigger", "--topic", "products/update"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains(
            "error: lsf sends webhooks to this machine or to --app-url only: \
             https://my-app.example.com/webhooks/products is neither; "
        ),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains(advice), "{}", text(&output));
    assert!(!text(&output).contains("start lsf"), "{}", text(&output));
    // An absolute `uri` on another host, `--app-url` or not.
    let output = lsf(
        &app.0,
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "orders/create",
            "--app-url",
            &url,
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains("https://hooks.example.net/orders is neither"),
        "{}",
        text(&output)
    );
    // With `--app-url`, the relative one goes there.
    let output = lsf(
        &app.0,
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "products/update",
            "--app-url",
            &url,
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    let received = received.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, "/webhooks/products");
}

/// A name under `localhost` is this machine (RFC 6761): sent without `--app-url`, to the
/// loopback whatever the system resolver says, with its `Host`.
#[test]
fn a_name_under_localhost_is_this_machine() {
    let (url, received) = backend(200);
    let port = url.rsplit(':').next().unwrap();
    let app = App::new(
        "localhost-name",
        &format!(
            r#"
client_id = "abc"
application_url = "http://app.localhost:{port}"

[webhooks]
api_version = "2026-10"

  [[webhooks.subscriptions]]
  topics = ["products/update"]
  uri = "/webhooks/products"
"#
        ),
    );
    let output = lsf(
        &app.0,
        &["app", "webhook", "trigger", "--topic", "products/update"],
    );
    assert!(output.status.success(), "{}", text(&output));
    let address = format!("http://hooks.localhost:{port}/direct");
    let output = lsf(
        &app.0,
        &[
            "app",
            "webhook",
            "trigger",
            "--topic",
            "products/update",
            "--address",
            &address,
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    let received = received.lock().unwrap();
    let seen: Vec<(&str, Option<&str>)> = received
        .iter()
        .map(|(path, headers, _)| (path.as_str(), header(headers, "host")))
        .collect();
    let (app_host, hooks_host) = (
        format!("app.localhost:{port}"),
        format!("hooks.localhost:{port}"),
    );
    assert_eq!(
        seen,
        [
            ("/webhooks/products", Some(app_host.as_str())),
            ("/direct", Some(hooks_host.as_str())),
        ]
    );
}
