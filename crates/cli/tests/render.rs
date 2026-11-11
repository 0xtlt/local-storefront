//! `lsf render`: one page printed without a server, as a visitor with a session of its own.

use std::path::Path;
use std::process::{Command, Output};

fn render(args: &[&str]) -> Output {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    Command::new(env!("CARGO_BIN_EXE_lsf"))
        .arg("--theme")
        .arg(theme)
        .arg("render")
        .args(args)
        .env_remove("LSF_DATA")
        .env_remove("LSF_CUSTOMER")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_page_renders_with_a_cart_and_a_customer() {
    let empty = render(&["/cart"]);
    assert!(stdout(&empty).contains(" empty"), "{}", stdout(&empty));

    let output = render(&["/cart", "--cart", "ceramic-mug=2", "--cart", "TEE-BLK-M"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let page = stdout(&output);
    assert!(page.contains("2 x Ceramic Mug"), "{page}");
    assert!(page.contains("(3)"), "{page}");

    // The session of `PUT /__lsf/session`, with `--cart` on top.
    let output = render(&[
        "/cart",
        "--session",
        r#"{"cart": {"items": [{"variant": "ceramic-mug", "quantity": 4}]}}"#,
        "--cart",
        "ceramic-mug",
    ]);
    let page = stdout(&output);
    assert!(page.contains("4 x Ceramic Mug"), "{page}");
    assert!(page.contains("(5)"), "{page}");

    let output = render(&["/", "--customer", "default"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn a_session_that_names_nothing_of_the_data_is_refused() {
    let output = render(&["/", "--customer", "nobody@example.com"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("there is no customer with the email \"nobody@example.com\""),
        "{}",
        stderr(&output)
    );
    let output = render(&["/", "--cart", "NOPE"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let output = render(&["/", "--cart", "ceramic-mug=two"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let output = render(&["/", "--session", r#"{"customr": "default"}"#]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("unknown field `customr`"),
        "{}",
        stderr(&output)
    );
}
