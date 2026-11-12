pub mod app;
pub mod app_init;
pub mod check;
pub mod docs;
pub mod fmt;
pub mod function;
pub mod init;
pub mod profile;
pub mod render;
pub mod routes;
pub mod schema;
pub mod serve;
pub mod validate;
pub mod webhook;

/// The value of `--app-url`: an `http://` or `https://` address with a host, where webhooks, OAuth
/// redirects and the app proxy go. Refused when the option is read, rather than by every delivery
/// failing later (`localhost:3000` without its scheme).
pub fn app_url(text: &str) -> Result<String, String> {
    if lsf_core::urls::is_http_url(text) {
        Ok(text.to_string())
    } else {
        Err(format!(
            "`{text}` is not an http:// or https:// address: give the backend's, like http://localhost:3000"
        ))
    }
}

/// A value of `--admin-source`: `<url>=<file>`, the URL ([`lsf_app::admin::sources::stub_key`]) and
/// the file, which must be there when lsf starts; its path made absolute, so that it is the
/// same file wherever lsf runs from. The URL is what comes before the last `=`, so that its
/// query may have some.
pub fn admin_source(text: &str) -> Result<(String, std::path::PathBuf), String> {
    let Some((url, file)) = text.rsplit_once('=') else {
        return Err(format!(
            "`{text}` is not <url>=<file>, like https://cdn.example.com/theme.zip=./fixtures/theme.zip"
        ));
    };
    let url = lsf_app::admin::sources::stub_key(url)?;
    let file = crate::paths::given(file.trim());
    let path = std::path::Path::new(&*file);
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(format!("{} is not a file", path.display())),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    }
    let path = std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((url, path))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_admin_source_is_a_url_and_a_file() {
        let dir = std::env::temp_dir().join(format!("lsf-admin-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("theme.zip");
        std::fs::write(&file, b"zip").unwrap();
        let (url, path) = super::admin_source(&format!(
            "HTTPS://CDN.example.com/theme.zip?v=1={}",
            file.display()
        ))
        .unwrap();
        assert_eq!(url, "https://cdn.example.com/theme.zip?v=1");
        assert_eq!(path, file);
        for bad in [
            "https://cdn.example.com/theme.zip".to_string(),
            format!("ftp://cdn.example.com/theme.zip={}", file.display()),
            format!("https://cdn.example.com/theme.zip={}", dir.display()),
            format!(
                "https://cdn.example.com/theme.zip={}",
                dir.join("none").display()
            ),
        ] {
            assert!(super::admin_source(&bad).is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_app_url_is_an_http_address() {
        assert_eq!(
            super::app_url("http://localhost:3000").as_deref(),
            Ok("http://localhost:3000")
        );
        assert!(super::app_url("https://app.example.test/base").is_ok());
        for bad in ["localhost:3000", "http://", "ftp://x", "https:///path", ""] {
            assert!(super::app_url(bad).is_err(), "{bad}");
        }
    }
}
