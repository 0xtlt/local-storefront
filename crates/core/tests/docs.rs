//! Keeps `docs/data-reference.md` in sync with the data format.
//!
//! After changing `store/model.rs`, regenerate it with
//! `UPDATE_SNAPSHOTS=1 cargo test -p slt-core --test docs`.

use std::path::Path;

const HEADER: &str = "# Store data reference\n\n\
    Every type of the store data format. This file is generated from the JSON Schema the\n\
    validator uses (`slt docs --reference`); the guide is in [data-format.md](data-format.md).\n\n";

#[test]
fn the_reference_is_up_to_date() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/data-reference.md");
    let expected = format!(
        "{HEADER}{}",
        slt_core::store::docs::reference()
            .replace("\n### ", "\n## ")
            .replacen("### ", "## ", 1)
    );
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&path, &expected).expect("write the reference");
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "docs/data-reference.md is stale: run `UPDATE_SNAPSHOTS=1 cargo test -p slt-core --test docs`"
    );
}
