//! What the server prints when it starts: the lines of the Admin API only with an app, before
//! `ready:`, so that a tool that reads up to `ready:` has the token; and where the client secret
//! came from.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A copy of the fixture app of lsf-checkout, in a folder of the test's own.
struct App(PathBuf);

impl App {
    fn new(name: &str) -> App {
        let path = std::env::temp_dir().join(format!("lsf-banner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app"),
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_lsf"));
        command
            .args(args)
            .args(["--host", "127.0.0.1", "--port", "0", "--static", "--quiet"])
            .current_dir(std::env::temp_dir())
            .env_remove("LSF_APP")
            .env_remove("LSF_APP_URL")
            .env_remove("LSF_CLIENT_SECRET")
            .env_remove("SHOPIFY_API_SECRET")
            .env_remove("LSF_THEME")
            .env_remove("LSF_DATA")
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
        let mut stream = TcpStream::connect(&self.address).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n\r\n",
            self.address
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default()
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

#[test]
fn a_theme_without_an_app_prints_no_admin_webhook_or_secret_line() {
    let theme = theme();
    let server = Server::start(&["--theme", &theme, "serve"], &[]);
    // The Admin API is served all the same, with its token.
    let admin: serde_json::Value = serde_json::from_str(&server.get("/__lsf/admin")).unwrap();
    let token = admin["shops"][0]["token"].as_str().unwrap();
    assert!(token.starts_with("shpat_"), "{admin}");
    let said = server.said();
    for line in &said {
        let line = line.trim_start();
        assert!(
            !line.starts_with("admin:")
                && !line.starts_with("webhooks:")
                && !line.starts_with("X-Shopify-Access-Token")
                && !line.contains("client secret"),
            "{}",
            said.join("\n")
        );
    }
    assert!(!said.join("\n").contains(token), "{}", said.join("\n"));
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
