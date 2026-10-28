//! `lsf checkout --watch` asked to stop by `SIGTERM`, as a process manager or `kill` does: it
//! stops as on Ctrl-C, with the builds that run and the folder of the published modules.
//!
//! The server runs as a process of its own, with a temporary folder (`TMPDIR`) and a home of the
//! test's: the published modules and the record of the builds are written there, never in the
//! user's folders. Its build waits for a file the test never writes (30 s at most).
//!
//! A test that fails leaves nothing running: [`Run`] stops the server, then the build's process
//! group, which the build says itself (a server killed does not stop its builds).

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

const LONG: Duration = Duration::from_secs(60);

/// Set to a folder, the test fails once the build runs, and says there what it started: how
/// [`a_test_that_fails_leaves_nothing_running`] sees what a failure leaves.
const FAIL: &str = "LSF_WATCH_SIGNAL_FAIL_INTO";
/// Set, [`Run`] kills the server at once, as if it did not stop on SIGTERM: its build is left to
/// the guard.
const NO_SIGTERM: &str = "LSF_WATCH_SIGNAL_NO_SIGTERM";

/// The test's folder, removed at the end, and the server and its build, stopped if the test
/// fails.
struct Run {
    base: PathBuf,
    server: Option<Child>,
    /// Where the build writes its process group.
    build_group: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(server) = &mut self.server {
            // As a process manager would: SIGTERM, which stops the builds, then SIGKILL.
            if std::env::var_os(NO_SIGTERM).is_none() {
                // SAFETY: the server is the test's child, not waited for yet.
                unsafe { libc::kill(server.id() as i32, libc::SIGTERM) };
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while server.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = server.kill();
            let _ = server.wait();
        }
        // What the server did not stop: the build has a process group of its own.
        if let Some((build, group)) =
            std::fs::read_to_string(&self.build_group)
                .ok()
                .and_then(|said| {
                    let (build, group) = said.trim().split_once(' ')?;
                    Some((build.parse::<i32>().ok()?, group.parse::<i32>().ok()?))
                })
        {
            // SAFETY: signals to the build's group, only while the build is in it, and never to
            // the test's own group.
            unsafe {
                if group > 1 && group != libc::getpgrp() && libc::getpgid(build) == group {
                    libc::kill(-group, libc::SIGKILL);
                }
            }
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
        build_group: base.join("ctl/group"),
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
    // It says who it is and its process group, then waits for a go that never comes, and gives
    // up after 30 s.
    write(
        &ctl.join("build.sh"),
        "ctl=../../../ctl\n\
         echo \"$$ $(ps -o pgid= -p $$ | tr -d ' ')\" > $ctl/group.partial\n\
         mv $ctl/group.partial $ctl/group\n\
         echo $$ > $ctl/pid.partial\nmv $ctl/pid.partial $ctl/pid\n\
         i=0\n\
         while [ ! -f $ctl/go ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i + 1)); done\n",
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
    if let Some(into) = std::env::var_os(FAIL) {
        let into = PathBuf::from(into);
        std::fs::write(into.join("build"), build.to_string()).unwrap();
        std::fs::write(into.join("server"), pid.to_string()).unwrap();
        std::fs::write(into.join("base"), base.as_os_str().as_encoded_bytes()).unwrap();
        panic!("failing on purpose ({FAIL})");
    }

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

/// The test above, failing once its build runs: its server and its build are stopped, and its
/// folder removed. With SIGTERM to the server, and with SIGKILL only (the build's group is then
/// the guard's to stop).
#[test]
fn a_test_that_fails_leaves_nothing_running() {
    if std::env::var_os(FAIL).is_some() {
        return;
    }
    for sigterm in [true, false] {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let into =
            std::env::temp_dir().join(format!("lsf-watch-failing-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&into).unwrap();
        let mut test = Command::new(std::env::current_exe().unwrap());
        test.args([
            "watch_stops_its_builds_and_removes_its_modules_on_sigterm",
            "--exact",
            "--test-threads=1",
        ])
        .env(FAIL, &into)
        .env_remove(NO_SIGTERM)
        .stdin(Stdio::null());
        if !sigterm {
            test.env(NO_SIGTERM, "1");
        }
        let output = test.output().unwrap();
        let said = |name: &str| std::fs::read_to_string(into.join(name)).unwrap_or_default();
        let (build, server, base) = (said("build"), said("server"), said("base"));
        let _ = std::fs::remove_dir_all(&into);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{stdout}");
        assert!(stdout.contains("failing on purpose"), "{stdout}");
        let build: i32 = build.parse().expect("the build ran");
        let server: i32 = server.parse().expect("the server ran");
        // Reaped by the system once killed: soon, well before the build would give up (30 s).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(gone(build) && gone(server)) {
            assert!(
                Instant::now() < deadline,
                "left running (SIGTERM: {sigterm}): build {build} {}, server {server} {}",
                !gone(build),
                !gone(server)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!Path::new(&base).exists(), "{base} is left");
    }
}
