//! What the server prints when it starts: the parts it serves (`--only`), the lines of the Admin
//! API only when it serves it, before `ready:`, so that a tool that reads up to `ready:` has the
//! token; and where the client secret came from.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A copy of the fixture app of lsf-app, in a folder of the test's own.
struct App(PathBuf);

impl App {
    fn new(name: &str) -> App {
        let path = std::env::temp_dir().join(format!("lsf-banner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app"),
            &path,
        );
        App(path)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A server of the test, stopped when dropped.
struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
    /// What it printed to stderr up to its `ready:` line, that line included.
    before_ready: Vec<String>,
    /// Its address, from the `ready:` line.
    address: String,
}

impl Server {
    fn start(args: &[&str], env: &[(&str, &str)]) -> Server {
        Server::start_with(args, env, &["--static", "--quiet"])
    }

    /// A server that looks at its files before it answers, as `lsf serve` does without
    /// `--static`, and says what it reloads.
    fn watching(args: &[&str], env: &[(&str, &str)]) -> Server {
        Server::start_with(args, env, &[])
    }

    fn start_with(args: &[&str], env: &[(&str, &str)], mode: &[&str]) -> Server {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lsf"));
        command
            .args(args)
            .args(["--host", "127.0.0.1", "--port", "0"])
            .args(mode)
            .current_dir(std::env::temp_dir())
            .env_remove("LSF_APP")
            .env_remove("LSF_APP_URL")
            .env_remove("LSF_CLIENT_SECRET")
            .env_remove("SHOPIFY_API_SECRET")
            .env_remove("LSF_THEME")
            .env_remove("LSF_DATA")
            .env_remove("LSF_PRICING_PLANS")
            .env_remove("LSF_ONLY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = send.send(line);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut before_ready = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no `ready:` line:\n{}", before_ready.join("\n")));
            let ready = line.starts_with("ready:");
            before_ready.push(line);
            if ready {
                break;
            }
        }
        let ready = before_ready.last().unwrap();
        let address = ready
            .split_whitespace()
            .nth(1)
            .and_then(|url| url.strip_prefix("http://"))
            .unwrap()
            .trim_end_matches('/')
            .to_string();
        Server {
            child,
            lines,
            before_ready,
            address,
        }
    }

    /// The body of `GET path`.
    fn get(&self, path: &str) -> String {
        self.fetch(path).2
    }

    /// The status of `GET path`, its headers (names in lowercase) and its body.
    fn fetch(&self, path: &str) -> (u16, Vec<(String, String)>, String) {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n\r\n",
            self.address
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse().ok())
            .unwrap_or(0);
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        (status, headers, body.to_string())
    }

    /// The `X-LSF-Reason` of `GET path`, when it answers `404` with one.
    fn reason_of_404(&self, path: &str) -> Option<String> {
        let (status, headers, _) = self.fetch(path);
        assert_eq!(status, 404, "{path}");
        headers
            .into_iter()
            .find(|(name, _)| name == "x-lsf-reason")
            .map(|(_, value)| value)
    }

    /// The status of `POST path` with a JSON `body`.
    fn post(&self, path: &str, body: &str) -> u16 {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            self.address,
            body.len()
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
            .split_whitespace()
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or(0)
    }

    /// The next line it prints that starts with `prefix`, within 10 seconds.
    fn next_line(&self, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no line starting with `{prefix}`"));
            if line.starts_with(prefix) {
                return line;
            }
        }
    }

    /// Everything it printed so far.
    fn said(&self) -> Vec<String> {
        let mut all = self.before_ready.clone();
        all.extend(self.lines.try_iter());
        all
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn theme() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../theme/tests/fixtures/theme")
        .to_str()
        .unwrap()
        .to_string()
}

/// The lines a server prints only when it serves the Admin API.
const ADMIN_LINES: &[&str] = &[
    "admin:",
    "webhooks:",
    "X-Shopify-Access-Token",
    "client secret",
    "embedded:",
    "proxy:",
    "pricing:",
    "partner:",
    "polaris:",
];

/// Asserts that `server` printed none of [`ADMIN_LINES`].
fn assert_no_admin_line(server: &Server) {
    let said = server.said();
    for line in &said {
        let line = line.trim_start();
        assert!(
            !ADMIN_LINES.iter().any(|start| line.starts_with(start)),
            "{}",
            said.join("\n")
        );
    }
}

#[test]
fn a_theme_without_an_app_serves_no_admin_api_and_prints_no_admin_line() {
    let theme = theme();
    let server = Server::start(&["--theme", &theme, "serve"], &[]);
    assert!(
        server.before_ready.contains(&"serving: theme".to_string()),
        "{}",
        server.before_ready.join("\n")
    );
    assert_no_admin_line(&server);
    // The Admin API is not served: its routes say which `--only` would serve it.
    for path in [
        "/__lsf/admin",
        "/admin/api/2026-10/graphql.json",
        "/__lsf/webhooks",
    ] {
        assert_eq!(
            server.reason_of_404(path).as_deref(),
            Some(
                "the admin part is not served (serving: theme): `serve --app <dir> --only theme,admin` serves it"
            ),
            "{path}"
        );
    }
    // The storefront is.
    assert_eq!(server.fetch("/").0, 200);
}

#[test]
fn the_parts_served_are_said_before_ready_and_the_admin_lines_only_with_the_admin_part() {
    let theme = theme();
    let app = App::new("parts");
    let dir = app.0.to_str().unwrap();
    let serving = |server: &Server| {
        let at = server
            .before_ready
            .iter()
            .position(|line| line.starts_with("serving: "))
            .unwrap_or_else(|| panic!("no serving: line:\n{}", server.before_ready.join("\n")));
        assert!(at < server.before_ready.len() - 1);
        server.before_ready[at].clone()
    };

    let server = Server::start(&["--theme", &theme, "serve", "--app", dir], &[]);
    assert_eq!(serving(&server), "serving: theme, checkout, admin");
    drop(server);

    // The checkout alone: no line of the Admin API, whose routes say why they are not served.
    let server = Server::start(
        &[
            "--theme", &theme, "serve", "--app", dir, "--only", "checkout",
        ],
        &[],
    );
    assert_eq!(serving(&server), "serving: checkout");
    assert_no_admin_line(&server);
    assert!(
        !server
            .before_ready
            .iter()
            .any(|line| line.starts_with("theme:")),
        "{}",
        server.before_ready.join("\n")
    );
    assert_eq!(
        server.reason_of_404("/__lsf/admin").as_deref(),
        Some(
            "the admin part is not served (serving: checkout): `serve --only checkout,admin` serves it"
        )
    );
    assert_eq!(
        server.reason_of_404("/collections/all").as_deref(),
        Some(
            "the theme part is not served (serving: checkout): `serve --only theme,checkout` serves it"
        )
    );
    assert_eq!(server.fetch("/__lsf/checkouts").0, 200);
    drop(server);

    // The theme and the Admin API: its lines, and its status says what runs.
    let server = Server::start(
        &[
            "--theme",
            &theme,
            "serve",
            "--app",
            dir,
            "--only",
            "theme,admin",
        ],
        &[],
    );
    assert_eq!(serving(&server), "serving: theme, admin");
    assert!(
        server
            .before_ready
            .iter()
            .any(|line| line.starts_with("admin:  http://")),
        "{}",
        server.before_ready.join("\n")
    );
    let status: serde_json::Value = serde_json::from_str(&server.get("/__lsf/admin")).unwrap();
    assert_eq!(status["parts"], serde_json::json!(["theme", "admin"]));
    assert!(server.reason_of_404("/__lsf/checkouts").is_some());
    drop(server);

    // `app dev`, and the variable: `LSF_ONLY`.
    let server = Server::start(&["app", "dev", "--app", dir], &[]);
    assert_eq!(serving(&server), "serving: checkout, admin");
    drop(server);
    let server = Server::start(&["app", "dev", "--app", dir], &[("LSF_ONLY", "admin")]);
    assert_eq!(serving(&server), "serving: admin");
    assert_eq!(
        server.reason_of_404("/checkout").as_deref(),
        Some(
            "the checkout part is not served (serving: admin): `serve --only checkout,admin` serves it"
        )
    );
}

#[test]
fn without_the_theme_serve_needs_no_theme_directory() {
    let app = App::new("no-theme");
    let empty = std::env::temp_dir().join(format!("lsf-banner-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let server = Server::start(
        &[
            "--theme",
            empty.to_str().unwrap(),
            "serve",
            "--app",
            app.0.to_str().unwrap(),
            "--only",
            "checkout,admin",
        ],
        &[],
    );
    assert!(
        server
            .before_ready
            .contains(&"serving: checkout, admin".to_string())
    );
    assert_eq!(server.fetch("/").0, 200, "the home page of the checkout");
    drop(server);
    let _ = std::fs::remove_dir_all(&empty);
}

/// What `lsf <args>` says on stderr when it refuses to start, with `env`.
fn refusal(args: &[&str], env: &[(&str, &str)]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lsf"));
    command
        .args(args)
        .args(["--port", "0"])
        .current_dir(std::env::temp_dir())
        .env_remove("LSF_APP")
        .env_remove("LSF_ONLY")
        .env_remove("LSF_PRICING_PLANS")
        .env_remove("LSF_APP_URL")
        .env_remove("LSF_CLIENT_SECRET")
        .stdin(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(2), "{args:?}");
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

#[test]
fn a_part_without_its_app_app_dev_with_a_theme_and_an_option_of_a_part_that_is_off_are_refused() {
    let theme = theme();
    let app = App::new("refusals");
    let dir = app.0.to_str().unwrap();
    assert_eq!(
        refusal(&["--theme", &theme, "serve", "--only", "admin"], &[]),
        "error: `--only admin` needs the app: `--app DIR` (or LSF_APP)"
    );
    assert_eq!(
        refusal(&["--theme", &theme, "serve"], &[("LSF_ONLY", "checkout")]),
        "error: `LSF_ONLY=checkout` needs the app: `--app DIR` (or LSF_APP)"
    );
    assert_eq!(
        refusal(&["app", "dev", "--app", dir, "--only", "theme"], &[]),
        "error: `--only theme`: `lsf app dev` serves no theme (`lsf serve --app DIR --only theme` serves it with the app)"
    );
    assert_eq!(
        refusal(
            &[
                "--theme",
                &theme,
                "serve",
                "--app",
                dir,
                "--only",
                "checkout",
                "--pricing-plans",
                "p.toml"
            ],
            &[]
        ),
        "error: --pricing-plans needs the admin part, which --only leaves out"
    );
    // The same option from a variable of the environment is said and ignored: one exported
    // for `app dev` must not stop a `serve` of fewer parts.
    let server = Server::start(
        &[
            "--theme", &theme, "serve", "--app", dir, "--only", "checkout",
        ],
        &[("LSF_PRICING_PLANS", "p.toml")],
    );
    assert!(
        server.before_ready.iter().any(|line| line
            == "note: LSF_PRICING_PLANS (--pricing-plans) is set, but the admin part is not served, which --only leaves out: ignored"),
        "{}",
        server.before_ready.join("\n")
    );
    drop(server);
    let server = Server::start(
        &["--theme", &theme, "serve"],
        &[("LSF_APP_URL", "http://localhost:3000")],
    );
    assert!(
        server.before_ready.iter().any(|line| line
            == "note: LSF_APP_URL (--app-url) is set, but the admin part is not served, which needs the app: `--app DIR` (or LSF_APP): ignored"),
        "{}",
        server.before_ready.join("\n")
    );
    drop(server);
    assert_eq!(
        refusal(&["--theme", &theme, "serve", "--variant", "shop"], &[]),
        "error: --variant needs the checkout part, which needs the app: `--app DIR` (or LSF_APP)"
    );
    assert_eq!(
        refusal(
            &["--theme", &theme, "serve", "--app", dir, "--watch"],
            &[("LSF_ONLY", "theme,admin")]
        ),
        "error: --watch needs the checkout part, which LSF_ONLY leaves out"
    );
}

#[test]
fn an_app_prints_its_token_webhooks_and_secret_before_ready() {
    let app = App::new("app");
    std::fs::write(app.0.join(".env"), "SHOPIFY_API_SECRET=from-dotenv\n").unwrap();
    // A Function not built: the advice is lsf's own build.
    std::fs::remove_file(app.0.join("extensions/quantity-limit/dist/function.wasm")).unwrap();
    let dir = app.0.to_str().unwrap();
    let server = Server::start(
        &[
            "app",
            "dev",
            "--app",
            dir,
            "--app-url",
            "http://localhost:3999",
        ],
        &[],
    );
    let before = &server.before_ready;
    let text = before.join("\n");
    let at = |prefix: &str| {
        before
            .iter()
            .position(|line| line.trim_start().starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` line before `ready:`:\n{text}"))
    };
    let (admin, token, secret, webhooks) = (
        at("admin:"),
        at("X-Shopify-Access-Token: shpat_"),
        at("client secret:"),
        at("webhooks:"),
    );
    assert!(
        admin < token && token < secret && secret < webhooks,
        "{text}"
    );
    assert!(webhooks < before.len() - 1, "{text}");
    // Where the secret came from, never the value.
    assert_eq!(
        before[secret].trim(),
        format!(
            "client secret: from SHOPIFY_API_SECRET of {}",
            app.0.join(".env").display()
        )
    );
    assert!(!server.said().join("\n").contains("from-dotenv"), "{text}");
    assert!(
        before[webhooks].ends_with("→ http://localhost:3999"),
        "{text}"
    );
    assert!(
        text.contains("quantity-limit: no Wasm module at ",)
            && text.contains(": build it with `lsf app build quantity-limit` (or `--build`)"),
        "{text}"
    );
    assert!(!text.contains("shopify app build"), "{text}");
}

#[test]
fn the_secret_line_says_where_the_secret_came_from() {
    let app = App::new("secret");
    let dir = app.0.to_str().unwrap();
    let secret_line = |args: &[&str], env: &[(&str, &str)]| {
        let server = Server::start(&[&["app", "dev", "--app", dir][..], args].concat(), env);
        let line = server
            .before_ready
            .iter()
            .find(|line| line.trim_start().starts_with("client secret:"))
            .unwrap_or_else(|| panic!("{}", server.before_ready.join("\n")))
            .trim()
            .to_string();
        (line, server.said().join("\n"))
    };
    let (line, said) = secret_line(&["--client-secret", "given-secret"], &[]);
    assert_eq!(line, "client secret: from --client-secret");
    assert!(!said.contains("given-secret"), "{said}");
    let (line, said) = secret_line(&[], &[("SHOPIFY_API_SECRET", "env-secret")]);
    assert_eq!(
        line,
        "client secret: from SHOPIFY_API_SECRET of the environment"
    );
    assert!(!said.contains("env-secret"), "{said}");
    // lsf's own is no secret: its value is printed.
    let (line, _) = secret_line(&[], &[]);
    assert!(
        line.starts_with("client secret: lsf-local-client-secret   (lsf's own: "),
        "{line}"
    );
}

#[test]
fn the_counted_lines_are_singular_for_one() {
    let app = App::new("counted");
    let dir = app.0.to_str().unwrap();
    // One Function, one UI extension, one subscription.
    let config = std::fs::read_to_string(app.0.join("shopify.app.toml")).unwrap();
    std::fs::write(
        app.0.join("shopify.app.toml"),
        format!(
            "{config}\n[webhooks]\napi_version = \"2026-10\"\n\n  [[webhooks.subscriptions]]\n  \
             topics = [\"orders/create\"]\n  uri = \"/webhooks\"\n"
        ),
    )
    .unwrap();
    let server = Server::start(
        &[
            "app",
            "dev",
            "--app",
            dir,
            "--app-url",
            "http://localhost:3999",
            "--extension",
            "volume-discount",
            "--extension",
            "gift-message",
        ],
        &[],
    );
    let text = server.before_ready.join("\n");
    assert!(
        text.contains(&format!("app:    {dir} (1 Function, 1 UI extension)")),
        "{text}"
    );
    assert!(
        text.contains("webhooks: 1 subscription of shopify.app.toml → http://localhost:3999"),
        "{text}"
    );
    // Two of each: the plural.
    drop(server);
    let server = Server::start(
        &[
            "app",
            "dev",
            "--app",
            dir,
            "--extension",
            "volume-discount",
            "--extension",
            "quantity-limit",
        ],
        &[],
    );
    let text = server.before_ready.join("\n");
    assert!(
        text.contains(&format!("app:    {dir} (2 Functions, 0 UI extensions)")),
        "{text}"
    );
}

/// An app of the test's own: a `shopify.app.toml` and nothing else, as a backend's.
fn backend_app(name: &str) -> App {
    let path = std::env::temp_dir().join(format!("lsf-banner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("shopify.app.toml"),
        "client_id = \"abc\"\nname = \"Backend\"\n",
    )
    .unwrap();
    App(path)
}

#[test]
fn an_app_without_extensions_says_it_is_a_backend_in_one_calm_line() {
    let app = backend_app("backend");
    let dir = app.0.to_str().unwrap();
    let server = Server::start(&["app", "dev", "--app", dir], &[]);
    let text = server.before_ready.join("\n");
    assert!(
        server.before_ready.contains(&format!(
            "app:    {dir} (no extensions: the Admin API, webhooks and the app proxy)"
        )),
        "{text}"
    );
    // Nothing that reads like an error: no warning of the directory, no scenarios to run.
    assert!(!text.contains("has no `extensions` directory"), "{text}");
    assert!(!text.contains("scenarios:"), "{text}");
}

#[test]
fn no_functions_serves_the_ui_extensions_alone() {
    let app = App::new("no-functions");
    let dir = app.0.to_str().unwrap();
    let server = Server::start(&["app", "dev", "--app", dir, "--no-functions"], &[]);
    let text = server.before_ready.join("\n");
    assert!(
        server.before_ready.contains(&format!(
            "app:    {dir} (2 UI extensions, no Function: --no-functions)"
        )),
        "{text}"
    );
    assert!(!text.contains("scenarios:"), "{text}");
    drop(server);

    // A Function named with `--extension` would not run: said before anything starts.
    let output = Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args(["app", "dev", "--app", dir, "--no-functions"])
        .args(["--extension", "quantity-limit", "--port", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--extension quantity-limit is a Function, and --no-functions runs none"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_webhook_the_backend_refuses_is_said_with_the_secret_lsf_signed_it_with() {
    // A backend that refuses every webhook, as one with another secret does.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://localhost:{}/webhooks",
        listener.local_addr().unwrap().port()
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let length = String::from_utf8_lossy(&head)
                .to_ascii_lowercase()
                .lines()
                .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = stream.read_exact(&mut body);
            let _ = write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\ncontent-length: 8\r\nconnection: close\r\n\r\nbad hmac"
            );
        }
    });
    let app = backend_app("refused");
    let dir = app.0.to_str().unwrap();
    let server = Server::start(
        &[
            "app",
            "dev",
            "--app",
            dir,
            "--client-secret",
            "given-secret",
        ],
        &[],
    );
    let status = server.post(
        "/__lsf/webhooks/trigger",
        &format!(r#"{{"topic": "app/uninstalled", "address": "{url}"}}"#),
    );
    assert_eq!(status, 200);
    // Said at once, `--quiet` or not, with where the secret came from and never its value.
    assert_eq!(
        server.next_line("webhooks: app/uninstalled"),
        format!(
            "webhooks: app/uninstalled → {url} answered 401: lsf signed it with the client secret \
             from --client-secret: does the backend verify with the same one?"
        )
    );
    assert!(!server.said().join("\n").contains("given-secret"));
}

#[test]
fn polaris_offline_is_said_before_ready_and_set_by_the_option_or_its_variable() {
    let theme = theme();
    // Polaris is the Admin API's: served with an app.
    let app = App::new("polaris");
    let dir = app.0.to_str().unwrap();
    // Online by default: the CDN's, nothing said.
    let server = Server::start(&["--theme", &theme, "serve", "--app", dir], &[]);
    assert!(
        !server
            .said()
            .iter()
            .any(|line| line.starts_with("polaris:"))
    );
    let status: serde_json::Value = serde_json::from_str(&server.get("/__lsf/polaris")).unwrap();
    assert_eq!(
        (status["mode"].as_str(), status["default"].as_str()),
        (Some("online"), Some("online"))
    );
    drop(server);

    for (args, env) in [
        (
            vec![
                "--theme",
                &theme,
                "serve",
                "--app",
                dir,
                "--polaris",
                "offline",
            ],
            vec![],
        ),
        (
            vec!["--theme", &theme, "serve", "--app", dir],
            vec![("LSF_POLARIS", "offline")],
        ),
    ] {
        let server = Server::start(&args, &env);
        let line = server
            .before_ready
            .iter()
            .find(|line| line.starts_with("polaris:"))
            .unwrap_or_else(|| {
                panic!(
                    "no polaris: line ({args:?} {env:?}):\n{}",
                    server.before_ready.join("\n")
                )
            });
        assert_eq!(
            line,
            &format!(
                "polaris: offline: http://{}/__lsf/cdn/shopifycloud/polaris-2.js (lsf's copy of Polaris 2.0.0 and 1.1.0)",
                server.address
            )
        );
        let status: serde_json::Value =
            serde_json::from_str(&server.get("/__lsf/polaris")).unwrap();
        assert_eq!(
            (status["mode"].as_str(), status["default"].as_str()),
            (Some("offline"), Some("offline"))
        );
        let script = server.get("/__lsf/cdn/shopifycloud/polaris-2.js");
        assert!(script.contains(&format!(
            "\"http://{}/__lsf/cdn/shopifycloud/admin-ui-foundations/icons/\"",
            server.address
        )));
    }
}

#[test]
fn pricing_plans_are_read_from_the_file_of_the_option_or_its_variable_and_said_before_ready() {
    let theme = theme();
    let dir = std::env::temp_dir().join(format!("lsf-banner-pricing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pricing.toml");
    std::fs::write(
        &file,
        "[[plans]]\nhandle = \"free\"\nname = \"Free\"\nprice = 0\n\n\
         [[plans]]\nhandle = \"pro\"\nname = \"Pro\"\nprice = 4.99\ntrial_days = 7\n",
    )
    .unwrap();
    let path = file.to_str().unwrap();
    // Managed pricing is the Admin API's: served with an app.
    let app = App::new("pricing-app");
    let app_dir = app.0.to_str().unwrap();
    // Without the option: no plans, nothing said.
    let server = Server::start(&["--theme", &theme, "serve", "--app", app_dir], &[]);
    assert!(
        !server
            .said()
            .iter()
            .any(|line| line.starts_with("pricing:"))
    );
    drop(server);

    for (args, env) in [
        (
            vec![
                "--theme",
                &theme,
                "serve",
                "--app",
                app_dir,
                "--pricing-plans",
                path,
            ],
            vec![],
        ),
        (
            vec!["--theme", &theme, "serve", "--app", app_dir],
            vec![("LSF_PRICING_PLANS", path)],
        ),
    ] {
        let server = Server::start(&args, &env);
        let line = server
            .before_ready
            .iter()
            .find(|line| line.starts_with("pricing:"))
            .unwrap_or_else(|| panic!("no pricing: line:\n{}", server.before_ready.join("\n")));
        assert_eq!(
            line,
            &format!(
                "pricing: 2 plans ({path}): http://{}/admin/charges/checkout-fixtures/pricing_plans   (managed pricing: the Billing API creates no charge)",
                server.address
            )
        );
        let status: serde_json::Value =
            serde_json::from_str(&server.get("/__lsf/admin/pricing-plans")).unwrap();
        assert_eq!(status["source"], serde_json::json!(path));
        assert_eq!(status["plans"][1]["trial_days"], serde_json::json!(7));
        // Where the app reads its plan: the Partner API, after the plans.
        let partner: serde_json::Value =
            serde_json::from_str(&server.get("/__lsf/admin/partner")).unwrap();
        let at = server
            .before_ready
            .iter()
            .position(|line| line.starts_with("pricing:"))
            .unwrap();
        assert_eq!(
            server.before_ready[at + 1],
            format!(
                "partner: {}   (Partner API: activeSubscription and events of {}; token {})",
                partner["url"].as_str().unwrap(),
                partner["app"]["id"].as_str().unwrap(),
                partner["token"].as_str().unwrap()
            )
        );
        let endpoint = partner["url"]
            .as_str()
            .unwrap()
            .split_once(&server.address)
            .unwrap()
            .1
            .to_string();
        // Routed to the Partner API (its 401 without a token), not to the storefront.
        assert_eq!(
            server.post(&endpoint, r#"{"query": "{ __typename }"}"#),
            401
        );
    }

    // A wrong file stops the start, and says why.
    std::fs::write(&file, "[[plans]]\nhandle = \"pro\"\nname = \"Pro\"\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args([
            "--theme",
            &theme,
            "serve",
            "--app",
            app_dir,
            "--pricing-plans",
            path,
            "--port",
            "0",
        ])
        .env_remove("LSF_PRICING_PLANS")
        .env_remove("LSF_ONLY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "--pricing-plans: {path}: plans[0] (pro): price is required (0, or free = true, for a free plan)"
        )),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_plans_file_is_read_again_before_a_control_request_when_it_changed() {
    let theme = theme();
    let dir = std::env::temp_dir().join(format!("lsf-banner-pricing-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pricing.toml");
    std::fs::write(
        &file,
        "[[plans]]\nhandle = \"pro\"\nname = \"Pro\"\nprice = 5\n",
    )
    .unwrap();
    let path = file.to_str().unwrap();
    let app = App::new("pricing-watch-app");
    let server = Server::watching(
        &[
            "--theme",
            &theme,
            "serve",
            "--app",
            app.0.to_str().unwrap(),
            "--pricing-plans",
            path,
        ],
        &[],
    );
    let price = |server: &Server| -> serde_json::Value {
        let status: serde_json::Value =
            serde_json::from_str(&server.get("/__lsf/admin/pricing-plans")).unwrap();
        status["plans"][0]["price"].clone()
    };
    assert_eq!(price(&server), serde_json::json!("5.00"));
    // A control request, with no page asked in between, sees the file as it is now (another
    // size too, for a file system whose times are coarse).
    std::fs::write(
        &file,
        "[[plans]]\nhandle = \"pro\"\nname = \"Pro\"\nprice = 7.5\n",
    )
    .unwrap();
    assert_eq!(price(&server), serde_json::json!("7.50"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !server
        .said()
        .iter()
        .any(|line| line.starts_with("pricing plans reloaded ("))
    {
        assert!(Instant::now() < deadline, "{}", server.said().join("\n"));
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}
