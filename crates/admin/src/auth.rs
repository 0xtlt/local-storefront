//! Tokens, OAuth, token exchange, session tokens, access scopes and the HMACs of Shopify.
//!
//! See `crates/admin/docs/design.md`.

pub mod oauth;
pub mod scopes;
pub mod session_token;
pub mod signing;
pub mod tokens;

pub use oauth::{AppAuth, Context, Shops};
pub use scopes::{RequiredAccess, Root, covers, required_access};
pub use signing::{app_proxy_signature, query_hmac, webhook_hmac};
pub use tokens::{Grant, Tokens, default_token};

use std::path::Path;

/// The client secret lsf signs with when nothing gives one: webhooks, OAuth and session tokens
/// verify against it, so a backend that reads its secret from the same place agrees with lsf.
pub const DEFAULT_CLIENT_SECRET: &str = "lsf-local-client-secret";

/// Where the client secret came from, for the server to say which one it signs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretSource {
    /// `--client-secret`, or `LSF_CLIENT_SECRET`.
    Explicit,
    /// `SHOPIFY_API_SECRET` of the environment: what Shopify's CLI gives the backend.
    Environment,
    /// `SHOPIFY_API_SECRET` of the app's `.env`.
    DotEnv,
    /// [`DEFAULT_CLIENT_SECRET`].
    Default,
}

/// The client secret of the app: the one given explicitly, else `SHOPIFY_API_SECRET` of the
/// environment, else that of the `.env` at the root of the app, else
/// [`DEFAULT_CLIENT_SECRET`].
pub fn client_secret(explicit: Option<&str>, app_root: Option<&Path>) -> (String, SecretSource) {
    if let Some(secret) = explicit.filter(|secret| !secret.is_empty()) {
        return (secret.to_string(), SecretSource::Explicit);
    }
    if let Ok(secret) = std::env::var("SHOPIFY_API_SECRET")
        && !secret.is_empty()
    {
        return (secret, SecretSource::Environment);
    }
    if let Some(secret) =
        app_root.and_then(|root| dotenv_value(&root.join(".env"), "SHOPIFY_API_SECRET"))
    {
        return (secret, SecretSource::DotEnv);
    }
    (DEFAULT_CLIENT_SECRET.to_string(), SecretSource::Default)
}

/// The value of `key` in a `.env` file: `KEY=value`, `export KEY=value`, quoted or not. A line
/// starting with `#` is a comment.
fn dotenv_value(file: &Path, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(file).ok()?;
    content.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let (name, value) = line.split_once('=')?;
        if name.trim() != key {
            return None;
        }
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or_else(|| value.split(" #").next().unwrap_or(value).trim());
        (!unquoted.is_empty()).then(|| unquoted.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_secret_of_a_dotenv_file() {
        let dir = std::env::temp_dir().join(format!("lsf-admin-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".env");
        std::fs::write(
            &file,
            "# the app\nSHOPIFY_API_KEY=abc\nexport SHOPIFY_API_SECRET=\"s3cr3t\"\n",
        )
        .unwrap();
        assert_eq!(
            dotenv_value(&file, "SHOPIFY_API_SECRET").as_deref(),
            Some("s3cr3t")
        );
        assert_eq!(
            dotenv_value(&file, "SHOPIFY_API_KEY").as_deref(),
            Some("abc")
        );
        assert_eq!(dotenv_value(&file, "MISSING"), None);
        assert_eq!(
            client_secret(Some("given"), Some(&dir)),
            ("given".to_string(), SecretSource::Explicit)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
