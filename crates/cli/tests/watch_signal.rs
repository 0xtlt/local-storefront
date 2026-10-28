//! `lsf checkout --watch` asked to stop by `SIGTERM`, as a process manager or `kill` does: it
//! stops as on Ctrl-C, with the builds that run and the folder of the published modules.
//!
//! The server runs as a process of its own, with a temporary folder (`TMPDIR`) and a home of the
//! test's: the published modules and the record of the builds are written there, never in the
//! user's folders. Its build waits for a file the test never writes (30 s at most).
//!
//! A test that fails leaves nothing running: [`Run`] stops the servers, then the builds' process
//! groups, which the builds say themselves (a server killed does not stop its builds).

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

/// The test's folder, removed at the end, and the servers and their builds, stopped if the test
/// fails.
struct Run {
    base: PathBuf,
    server: Option<Child>,
    /// Other servers, for the tests that start more than one.
    others: Vec<Child>,
    /// Where the builds write their process group: `group`, or `group-<n>` for build `n`.
    build_groups: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        for server in self.server.iter_mut().chain(self.others.iter_mut()) {
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
        // What the servers did not stop: each build has a process group of its own.
        let said = std::fs::read_dir(&self.build_groups)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name == "group" || name.starts_with("group-") && !name.ends_with(".partial")
            })
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok());
        for (build, group) in said.filter_map(|said| {
            let (build, group) = said.trim().split_once(' ')?;
            Some((build.parse::<i32>().ok()?, group.parse::<i32>().ok()?))
        }) {
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
        others: Vec::new(),
        build_groups: base.join("ctl"),
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

/// What a build that waits for the write lock of its Function says.
const WAITING: &str = "waiting for another build of it to end";

/// A module that exports a function, and whose last section (a custom one) holds `text`.
fn module(text: &str) -> Vec<u8> {
    let mut bytes = b"\0asm\x01\0\0\0\
        \x01\x04\x01\x60\0\0\
        \x03\x02\x01\0\
        \x07\x05\x01\x01f\0\0\
        \x0a\x04\x01\x02\0\x0b"
        .to_vec();
    bytes.extend([0, text.len() as u8 + 1, 0]);
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// The file named `name` under `dir`, at any depth.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if entry.file_name() == name {
            return Some(path);
        }
    }
    None
}

/// A server of the test, and what it printed.
struct Server {
    pid: i32,
    lines: mpsc::Receiver<String>,
    said: Vec<String>,
}

impl Server {
    /// Starts `lsf checkout --app <base>/app --watch` (with `--build`), with the test's
    /// temporary folder and home. `run` stops it if the test fails.
    fn start(run: &mut Run, build: bool) -> Server {
        let base = &run.base;
        let mut command = Command::new(env!("CARGO_BIN_EXE_lsf"));
        command
            .args(["checkout", "--app"])
            .arg(base.join("app"))
            .args(["--watch", "--host", "127.0.0.1", "--port", "0"])
            .current_dir(base)
            .env("TMPDIR", base.join("tmp"))
            .env("HOME", base.join("home"))
            .env("XDG_CACHE_HOME", base.join("home/.cache"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if build {
            command.arg("--build");
        }
        let mut server = command.spawn().unwrap();
        let (lines, said) = mpsc::channel();
        let stderr = server.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = lines.send(line);
            }
        });
        let pid = server.id() as i32;
        run.others.push(server);
        Server {
            pid,
            lines: said,
            said: Vec::new(),
        }
    }

    fn said(&mut self) -> String {
        self.said.extend(self.lines.try_iter());
        self.said.join("\n")
    }

    fn wait_said(&mut self, what: &str) {
        let deadline = Instant::now() + LONG;
        while !self.said().contains(what) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what:?}:\n{}",
                self.said()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends `signal` to the server and waits for it to be over, `within` at most.
    fn stop(&mut self, run: &mut Run, signal: i32, within: Duration) -> std::process::ExitStatus {
        // SAFETY: the server is the test's child, not waited for yet.
        assert_eq!(unsafe { libc::kill(self.pid, signal) }, 0);
        let at = run
            .others
            .iter()
            .position(|server| server.id() as i32 == self.pid)
            .unwrap();
        let mut server = run.others.remove(at);
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = server.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                run.others.push(server);
                panic!("the server did not stop in {within:?}:\n{}", self.said());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A server killed during a build (SIGKILL: it cannot stop it) leaves it running, and that build
/// still holds the write lock of its Function: another server's build of it waits for it to be
/// over, then builds, and publishes and records its own module, not the one the build left
/// running wrote. SIGTERM stops a server whose build waits.
#[test]
fn a_build_left_running_by_a_killed_server_keeps_its_lock() {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base =
        std::env::temp_dir().join(format!("lsf-watch-orphan-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let mut run = Run {
        base: base.clone(),
        server: None,
        others: Vec::new(),
        build_groups: base.join("ctl"),
    };
    let app = base.join("app");
    let ctl = base.join("ctl");
    let tmp = base.join("tmp");
    for folder in [&ctl, &tmp, &base.join("home")] {
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
    write(&app.join("extensions/fn-a/src/main.rs"), "// v1\n");
    std::fs::create_dir_all(app.join("extensions/fn-a/dist")).unwrap();
    std::fs::write(app.join("extensions/fn-a/dist/function.wasm"), module("v0")).unwrap();
    // Build `n` says who it is and its process group, waits for `go-<n>` (30 s at most, and
    // not once the test's folder is gone), then writes `module-<n>`.
    write(
        &ctl.join("build.sh"),
        "ctl=../../../ctl\n\
         echo build >> $ctl/count\n\
         n=$(wc -l < $ctl/count | tr -d ' ')\n\
         echo \"$$ $(ps -o pgid= -p $$ | tr -d ' ')\" > $ctl/group-$n.partial\n\
         mv $ctl/group-$n.partial $ctl/group-$n\n\
         mkdir -p dist\n\
         i=0\n\
         while [ ! -f $ctl/go-$n ] && [ -d $ctl ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i + 1)); done\n\
         cp $ctl/module-$n dist/function.wasm\n",
    );
    // A's build makes the module of the files of then; the next build, that of the files now.
    std::fs::write(ctl.join("module-1"), module("old")).unwrap();
    std::fs::write(ctl.join("module-2"), module("new")).unwrap();
    std::fs::write(ctl.join("go-2"), "").unwrap();
    let builds = || {
        std::fs::read_to_string(ctl.join("count"))
            .unwrap_or_default()
            .lines()
            .count()
    };

    // A builds at start, and is killed meanwhile: its build goes on.
    let mut a = Server::start(&mut run, true);
    wait_for("A's build", || ctl.join("group-1").is_file());
    let orphan: i32 = std::fs::read_to_string(ctl.join("group-1"))
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    a.stop(&mut run, libc::SIGKILL, Duration::from_secs(5));
    assert!(!gone(orphan));
    // Its lock says who held it: A, and the build's process group.
    let said =
        std::fs::read_to_string(find(&base.join("home"), "fn-a.lock").expect("the lock")).unwrap();
    let group = std::fs::read_to_string(ctl.join("group-1")).unwrap();
    let group = group.trim().split(' ').nth(1).unwrap();
    assert!(said.starts_with(&format!("held {} ", a.pid)), "{said}");
    assert!(said.contains(&format!("\ngroup {group}\n")), "{said}");
    assert!(!said.contains("released"), "{said}");

    // The files change. B's build waits for A's; SIGTERM stops B at once all the same.
    write(&app.join("extensions/fn-a/src/main.rs"), "// v2\n");
    let mut b = Server::start(&mut run, false);
    b.wait_said(WAITING);
    let status = b.stop(&mut run, libc::SIGTERM, Duration::from_secs(5));
    assert!(status.success(), "{status}:\n{}", b.said());
    assert!(!tmp.join(format!("lsf-watch-modules-{}-0", b.pid)).exists());
    assert_eq!(builds(), 1);

    // C's waits too, then builds once A's is over, and publishes its own module.
    let mut c = Server::start(&mut run, false);
    c.wait_said(WAITING);
    assert_eq!(builds(), 1);
    assert!(!gone(orphan));
    std::fs::write(ctl.join("go-1"), "").unwrap();
    wait_for("A's build to end", || gone(orphan));
    c.wait_said("✓ 1 Function built");
    assert_eq!(builds(), 2);
    let published = |server: &Server| {
        std::fs::read(tmp.join(format!("lsf-watch-modules-{}-0/fn-a.wasm", server.pid)))
            .unwrap_or_default()
    };
    assert_eq!(published(&c), module("new"), "{}", c.said());
    assert_eq!(
        std::fs::read(app.join("extensions/fn-a/dist/function.wasm")).unwrap(),
        module("new")
    );
    let status = c.stop(&mut run, libc::SIGTERM, Duration::from_secs(5));
    assert!(status.success(), "{status}:\n{}", c.said());

    // What C recorded is what it built: the next server trusts it, and builds nothing.
    let mut d = Server::start(&mut run, false);
    d.wait_said("ready:");
    assert_eq!(builds(), 2, "{}", d.said());
    assert!(!d.said().contains("building"), "{}", d.said());
    assert_eq!(published(&d), module("new"));
    let status = d.stop(&mut run, libc::SIGTERM, Duration::from_secs(5));
    assert!(status.success(), "{status}:\n{}", d.said());
}
