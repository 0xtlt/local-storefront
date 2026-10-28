//! `lsf checkout --watch` asked to stop by `SIGTERM`, as a process manager or `kill` does: it
//! stops as on Ctrl-C, with the builds that run and the folder of the published modules.
//!
//! The server runs as a process of its own, with a temporary folder (`TMPDIR`) and a home of the
//! test's: the published modules and the record of the builds are written there, never in the
//! user's folders. Its build waits for a file the test never writes.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

const LONG: Duration = Duration::from_secs(60);

/// The test's folder, removed at the end, and the server, stopped if the test fails.
struct Run {
    base: PathBuf,
    server: Option<Child>,
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(server) = &mut self.server {
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LONG;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the process `pid` is gone.
fn gone(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) != 0 }
}

#[test]
fn watch_stops_its_builds_and_removes_its_modules_on_sigterm() {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base =
        std::env::temp_dir().join(format!("lsf-watch-sigterm-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let mut run = Run {
        base: base.clone(),
        server: None,
    };
    let app = base.join("app");
    let ctl = base.join("ctl");
    let tmp = base.join("tmp");
    let home = base.join("home");
    for folder in [&ctl, &tmp, &home] {
        std::fs::create_dir_all(folder).unwrap();
    }
    write(
        &app.join("shopify.app.toml"),
        "name = \"Watch\"\nhandle = \"watch\"\n",
    );
    write(
        &app.join("extensions/fn-a/shopify.extension.toml"),
        r#"api_version = "2026-10"

[[extensions]]
name = "fn-a"
handle = "fn-a"
type = "function"

  [[extensions.targeting]]
  target = "cart.lines.discounts.generate.run"
  input_query = "src/run.graphql"
  export = "cart_lines_discounts_generate_run"

  [extensions.build]
  command = "sh ../../../ctl/build.sh"
  path = "dist/function.wasm"
"#,
    );
    write(
        &app.join("extensions/fn-a/src/run.graphql"),
        "query { cart { cost { subtotalAmount { amount } } } }\n",
    );
    // It says who it is, then waits for a go that never comes.
    write(
        &ctl.join("build.sh"),
        "echo $$ > ../../../ctl/pid.partial\nmv ../../../ctl/pid.partial ../../../ctl/pid\n\
         while [ ! -f ../../../ctl/go ]; do sleep 0.05; done\n",
    );

    let mut server = Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args(["checkout", "--app"])
        .arg(&app)
        .args(["--watch", "--build", "--host", "127.0.0.1", "--port", "0"])
        .current_dir(&base)
        .env("TMPDIR", &tmp)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = server.id() as i32;
    let (lines, said) = mpsc::channel();
    let stderr = server.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = lines.send(line);
        }
    });
    run.server = Some(server);
    let output = || said.try_iter().collect::<Vec<_>>().join("\n");

    // The server is up, its build runs, its modules have a folder.
    let modules = tmp.join(format!("lsf-watch-modules-{pid}-0"));
    wait_for("the build to start", || ctl.join("pid").is_file());
    wait_for("the folder of the modules", || modules.is_dir());
    let build: i32 = std::fs::read_to_string(ctl.join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!gone(build));

    // SAFETY: the server is the test's child.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let server = run.server.as_mut().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the server did not stop:\n{}",
            output()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    run.server = None;
    assert!(status.success(), "{status}:\n{}", output());
    wait_for("the build to be stopped", || gone(build));
    assert!(!modules.exists(), "the published modules are left");
}
