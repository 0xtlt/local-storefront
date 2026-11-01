//! The parts of the HTTP side that do not need a shop: reading a GraphQL request, the throttle
//! setting. `tests/api.rs` runs the endpoints end to end.

use serde_json::json;

use super::HttpRequest;
use super::admin::{ThrottleSetting, graphql_request};
use crate::throttle::Plan;

fn post(content_type: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/admin/api/2026-10/graphql.json".into(),
        headers: [("content-type".to_string(), content_type.to_string())].into(),
        body: body.as_bytes().to_vec(),
        ..HttpRequest::default()
    }
}

#[test]
fn reads_the_graphql_request_of_every_form() {
    let json_body = graphql_request(&post(
        "application/json",
        r#"{"query": "query Q($n: Int) { shop { name } }", "variables": {"n": 1}, "operationName": "Q"}"#,
    ))
    .unwrap();
    assert_eq!(json_body.variables["n"], json!(1));
    assert_eq!(json_body.operation_name.as_deref(), Some("Q"));
    // Variables sent as a string of JSON, and an empty operation name.
    let string = graphql_request(&post(
        "application/json; charset=utf-8",
        r#"{"query": "{ shop { name } }", "variables": "{\"n\": 2}", "operationName": ""}"#,
    ))
    .unwrap();
    assert_eq!(string.variables["n"], json!(2));
    assert_eq!(string.operation_name, None);
    let raw = graphql_request(&post("application/graphql", "{ shop { name } }")).unwrap();
    assert_eq!(raw.query, "{ shop { name } }");
    let get = graphql_request(&HttpRequest {
        method: "GET".into(),
        query: vec![
            ("query".into(), "{ shop { name } }".into()),
            ("variables".into(), r#"{"a": true}"#.into()),
        ],
        ..HttpRequest::default()
    })
    .unwrap();
    assert_eq!(get.variables["a"], json!(true));

    assert!(graphql_request(&post("application/json", "{}")).is_err());
    assert!(graphql_request(&post("application/json", "not json")).is_err());
    assert!(
        graphql_request(&post(
            "application/json",
            r#"{"query": "{ a }", "variables": [1]}"#
        ))
        .is_err()
    );
}

#[test]
fn reads_the_throttle_setting() {
    assert_eq!(ThrottleSetting::parse("off"), Ok(ThrottleSetting::Off));
    assert_eq!(
        ThrottleSetting::parse("Plus"),
        Ok(ThrottleSetting::Plan(Plan::PLUS))
    );
    assert_eq!(ThrottleSetting::parse("store"), Ok(ThrottleSetting::Store));
    assert!(
        ThrottleSetting::parse("gold")
            .unwrap_err()
            .contains("off, store, standard")
    );
}

/// The Admin API of the demo store with the app of `tests/fixtures/platform-app`, its optional
/// scopes granted.
fn platform_api() -> std::sync::Arc<super::AdminApi> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/platform-app");
    let app = lsf_checkout::app::load(&root).expect("the app loads");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let api = super::AdminApi::new(
        &store,
        super::AdminOptions {
            app: Some(app),
            client_secret: "test-secret".to_string(),
            throttle: ThrottleSetting::Off,
            token_seed: Some(5),
            ..Default::default()
        },
    );
    let installed = send(
        &api,
        "POST",
        "/__lsf/admin/install",
        None,
        r#"{"scopes": "read_products,write_products,write_customers,read_discounts"}"#,
    );
    assert_eq!(installed.status, 200);
    std::sync::Arc::new(api)
}

fn send(
    api: &super::AdminApi,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> super::HttpResponse {
    let mut headers: std::collections::HashMap<String, String> =
        [("content-type".to_string(), "application/json".to_string())].into();
    if let Some(token) = token {
        headers.insert("x-shopify-access-token".to_string(), token.to_string());
    }
    api.handle(&HttpRequest {
        method: method.into(),
        path: path.into(),
        headers,
        body: body.as_bytes().to_vec(),
        ..HttpRequest::default()
    })
    .expect("routed")
}

fn graphql(api: &super::AdminApi, token: &str, query: &str) -> (u16, serde_json::Value) {
    let response = send(
        api,
        "POST",
        "/admin/api/2026-10/graphql.json",
        Some(token),
        &json!({ "query": query }).to_string(),
    );
    (
        response.status,
        serde_json::from_slice(&response.body).unwrap_or_default(),
    )
}

/// A request that waited on the shop while `appUninstall`, `appRevokeAccessScopes` or a reset
/// ran ran after it with the grant it had looked up before: each change of the tokens moves the
/// shop's auth generation, and a request that sees it moved looks its token up again.
#[test]
fn a_request_that_waited_on_the_shop_runs_with_the_tokens_as_they_are_now() {
    // Uninstalled while it waited: refused.
    let api = platform_api();
    let token = api.token(&api.default_shop());
    let other = api.clone();
    let uninstalling = token.clone();
    api.at_next(super::admin::Hook::LookedUp, move || {
        let (_, answer) = graphql(
            &other,
            &uninstalling,
            "mutation { appUninstall { app { id } userErrors { message } } }",
        );
        assert_eq!(
            answer["data"]["appUninstall"]["userErrors"],
            json!([]),
            "{answer}"
        );
    });
    let (status, answer) = graphql(
        &api,
        &token,
        r#"mutation { productCreate(product: {title: "After the uninstall"}) { product { id } } }"#,
    );
    assert_eq!(status, 401, "{answer}");
    assert_eq!(
        api.with_default_shop(|state| state
            .all("Product")
            .filter(|product| product["title"] == "After the uninstall")
            .count()),
        Some(0),
        "no product made after the uninstall"
    );

    // A scope revoked while it waited: the request runs with the scopes left.
    let api = platform_api();
    let token = api.token(&api.default_shop());
    let other = api.clone();
    let revoking = token.clone();
    api.at_next(super::admin::Hook::LookedUp, move || {
        let (_, answer) = graphql(
            &other,
            &revoking,
            "mutation { appRevokeAccessScopes(scopes: [\"write_customers\"]) { revoked { handle } userErrors { message } } }",
        );
        assert_eq!(
            answer["data"]["appRevokeAccessScopes"]["revoked"],
            json!([{"handle": "write_customers"}]),
            "{answer}"
        );
    });
    let (status, answer) = graphql(
        &api,
        &token,
        r#"mutation { customerCreate(input: {email: "late@example.com"}) { customer { id } } }"#,
    );
    assert_eq!(status, 200);
    assert_eq!(
        answer["errors"][0]["extensions"]["code"], "ACCESS_DENIED",
        "{answer}"
    );

    // A reset while it waited: a token OAuth issued is gone.
    let api = platform_api();
    let shop = api.default_shop();
    let (_, created) = graphql(
        &api,
        &api.token(&shop),
        "mutation { delegateAccessTokenCreate(input: {delegateAccessScope: [\"read_products\"]}) { delegateAccessToken { accessToken } } }",
    );
    let delegate =
        created["data"]["delegateAccessTokenCreate"]["delegateAccessToken"]["accessToken"]
            .as_str()
            .unwrap()
            .to_string();
    let other = api.clone();
    api.at_next(super::admin::Hook::LookedUp, move || {
        assert_eq!(send(&other, "DELETE", "/__lsf/admin", None, "").status, 200);
    });
    let (status, answer) = graphql(&api, &delegate, "{ products(first: 1) { nodes { id } } }");
    assert_eq!(status, 401, "{answer}");
}

/// A request that came in after `appUninstall` unlocked the shop but before its effects reached
/// the tokens waits for them: it is refused, not run with the token being revoked.
#[test]
fn a_request_waits_for_the_tokens_a_request_before_it_changed() {
    let api = platform_api();
    let token = api.token(&api.default_shop());
    let other = api.clone();
    let late = token.clone();
    let (sender, received) = std::sync::mpsc::channel();
    api.at_next(super::admin::Hook::Unlocked, move || {
        std::thread::spawn(move || {
            let answer = graphql(
                &other,
                &late,
                r#"mutation { productCreate(product: {title: "During the uninstall"}) { product { id } } }"#,
            );
            let _ = sender.send(answer);
        });
        // Time for it to reach the shop, which it must not run on yet.
        std::thread::sleep(std::time::Duration::from_millis(100));
    });
    let (_, answer) = graphql(
        &api,
        &token,
        "mutation { appUninstall { app { id } userErrors { message } } }",
    );
    assert_eq!(
        answer["data"]["appUninstall"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let (status, answer) = received
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the late request answers");
    assert_eq!(status, 401, "{answer}");
}

/// A server on the loopback that answers each request with `body` after `delay`: its URL.
fn slow_source(delay: std::time::Duration, body: &'static str) -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/slow.css", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                std::thread::sleep(delay);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    url
}

/// A `URL` body of `themeFilesUpsert` was downloaded within the mutation, under the shop's lock:
/// every request on the shop waited for it (50 of them, 500 s). It is fetched before the lock.
#[test]
fn a_source_downloads_while_the_shop_serves_other_requests() {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let api = std::sync::Arc::new(
        super::AdminApi::new(
            &store,
            super::AdminOptions {
                throttle: ThrottleSetting::Off,
                token_seed: Some(6),
                ..Default::default()
            },
        )
        // The slow source is on the loopback.
        .with_fetch(crate::sources::FetchOptions { allow_local: true }),
    );
    let token = api.token(&api.default_shop());
    let delay = std::time::Duration::from_millis(1500);
    let url = slow_source(delay, "body { color: red }");
    let upserting = {
        let (api, token) = (api.clone(), token.clone());
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let answer = graphql(
                &api,
                &token,
                &format!(
                    r#"mutation {{ themeFilesUpsert(themeId: "gid://shopify/OnlineStoreTheme/1", files: [{{filename: "assets/slow.css", body: {{type: URL, value: "{url}"}}}}]) {{ job {{ done }} userErrors {{ message }} }} }}"#
                ),
            );
            (answer, started.elapsed())
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    let started = std::time::Instant::now();
    let (status, shop) = graphql(&api, &token, "{ shop { name } }");
    let waited = started.elapsed();
    assert_eq!(status, 200, "{shop}");
    assert!(
        waited < std::time::Duration::from_millis(700),
        "a read of the shop waited {waited:?} on the download"
    );
    let ((status, answer), took) = upserting.join().unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        answer["data"]["themeFilesUpsert"]["userErrors"],
        json!([]),
        "{answer}"
    );
    assert_eq!(answer["data"]["themeFilesUpsert"]["job"]["done"], true);
    assert!(took >= delay, "the source was downloaded: {took:?}");
    let body = api
        .with_default_shop(|state| {
            let theme = state.get("gid://shopify/OnlineStoreTheme/1").cloned()?;
            crate::domains::themes::files::read(state, &theme, "assets/slow.css")
        })
        .flatten()
        .expect("the file was written");
    assert_eq!(&body.bytes[..], b"body { color: red }");
}

/// `themeCreate` from a URL on the loopback answers what an unreachable source answers, and the
/// URL is never connected to: Shopify cannot reach the developer's machine either.
#[test]
fn a_theme_is_not_created_from_the_developers_machine() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/theme.zip", listener.local_addr().unwrap());
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for _ in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let api = super::AdminApi::new(
        &store,
        super::AdminOptions {
            throttle: ThrottleSetting::Off,
            ..Default::default()
        },
    );
    let token = api.token(&api.default_shop());
    let (status, answer) = graphql(
        &api,
        &token,
        &format!(
            r#"mutation {{ themeCreate(source: "{url}") {{ theme {{ id }} userErrors {{ field message code }} }} }}"#
        ),
    );
    assert_eq!(status, 200);
    let error = &answer["data"]["themeCreate"]["userErrors"][0];
    assert_eq!(error["code"], "INVALID_ZIP", "{answer}");
    assert_eq!(error["field"], json!(["source"]));
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("a loopback address") && message.contains("--admin-fetch-local"),
        "{message}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the URL was never connected to"
    );
}

/// `PUT /__lsf/admin/sources?url=`, an `--admin-source` or a request's body: what `themeCreate`
/// and `urlRedirectImportCreate` read at a URL of the backend's own, which nothing serves in a
/// test; never connected to, on the loopback without `--admin-fetch-local`. Every shop's, kept
/// by a reset; `DELETE` forgets it, and the URL is refused again.
#[test]
fn a_stubbed_url_is_read_without_being_fetched() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for _ in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let dir = std::env::temp_dir().join(format!("lsf-api-stubs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("redirects.csv");
    std::fs::write(&csv, "Redirect from,Redirect to\n/old,/products/new\n").unwrap();
    let api = super::AdminApi::new(
        &store,
        super::AdminOptions {
            throttle: ThrottleSetting::Off,
            ..Default::default()
        },
    )
    .with_sources(crate::sources::Stubs::configured([(
        format!("{origin}/redirects.csv"),
        csv.clone(),
    )]));
    let token = api.token(&api.default_shop());
    let control = |method: &str, url: Option<&str>, body: Vec<u8>| {
        let response = api
            .handle(&HttpRequest {
                method: method.into(),
                path: "/__lsf/admin/sources".into(),
                query: url
                    .map(|url| vec![("url".to_string(), url.to_string())])
                    .unwrap_or_default(),
                body,
                ..HttpRequest::default()
            })
            .expect("routed");
        let json = serde_json::from_slice(&response.body).unwrap_or(json!(null));
        (response.status, json, response.body)
    };
    let theme_url = format!("{origin}/themes/comfort.zip?v=2");
    let archive = crate::domains::themes::zip::stored(&[
        (
            "layout/theme.liquid",
            b"<html>{{ content_for_header }}{{ content_for_layout }}</html>",
        ),
        ("templates/index.liquid", b"home"),
        (
            "config/settings_schema.json",
            br#"[{"name": "theme_info", "theme_name": "Comfort"}]"#,
        ),
    ]);
    let (status, put, _) = control("PUT", Some(&theme_url), archive.clone());
    assert_eq!(status, 200, "{put}");
    assert_eq!(
        put,
        json!({"ok": true, "url": theme_url, "size": archive.len()})
    );
    let create = |api: &super::AdminApi| {
        graphql(
            api,
            &token,
            &format!(
                r#"mutation {{ themeCreate(source: "{theme_url}") {{ theme {{ name }} userErrors {{ code message }} }} }}"#
            ),
        )
        .1
    };
    let created = create(&api);
    assert_eq!(
        created["data"]["themeCreate"]["theme"]["name"], "Comfort",
        "{created}"
    );
    // The configured file.
    let (_, import) = graphql(
        &api,
        &token,
        &format!(
            r#"mutation {{ urlRedirectImportCreate(url: "{origin}/redirects.csv") {{ urlRedirectImport {{ count }} userErrors {{ message }} }} }}"#
        ),
    );
    assert_eq!(
        import["data"]["urlRedirectImportCreate"]["urlRedirectImport"]["count"], 1,
        "{import}"
    );
    let (_, listed, _) = control("GET", None, Vec::new());
    assert_eq!(listed["count"], 2, "{listed}");
    assert_eq!(listed["sources"][0]["from"], csv.display().to_string());
    assert_eq!(listed["sources"][1]["from"], "request");
    let (status, _, bytes) = control("GET", Some(&theme_url), Vec::new());
    assert_eq!((status, bytes), (200, archive));
    // A reset keeps it: the internet is not the shop.
    assert_eq!(send(&api, "DELETE", "/__lsf/admin", None, "").status, 200);
    let token = api.token(&api.default_shop());
    let again = graphql(
        &api,
        &token,
        &format!(r#"mutation {{ themeCreate(source: "{theme_url}") {{ theme {{ name }} }} }}"#),
    )
    .1;
    assert_eq!(
        again["data"]["themeCreate"]["theme"]["name"], "Comfort",
        "{again}"
    );
    // Forgotten: the URL is the developer's machine again, refused.
    let (_, deleted, _) = control("DELETE", None, Vec::new());
    assert_eq!(deleted, json!({"ok": true, "removed": 1}));
    let refused = graphql(
        &api,
        &token,
        &format!(
            r#"mutation {{ themeCreate(source: "{theme_url}") {{ userErrors {{ code message }} }} }}"#
        ),
    )
    .1;
    let error = &refused["data"]["themeCreate"]["userErrors"][0];
    assert_eq!(error["code"], "INVALID_ZIP", "{refused}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("--admin-source"),
        "{refused}"
    );
    let (status, bad, _) = control("PUT", Some("file:///etc/passwd"), b"x".to_vec());
    assert_eq!(status, 422, "{bad}");
    assert_eq!(control("PUT", None, b"x".to_vec()).0, 422);
    assert_eq!(control("POST", None, Vec::new()).0, 405);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing was connected to");
    let _ = std::fs::remove_dir_all(dir);
}
