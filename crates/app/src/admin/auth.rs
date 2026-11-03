//! Tokens, OAuth, token exchange, session tokens, access scopes and the HMACs of Shopify.
//!
//! See `crates/app/docs/admin/design.md`.

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

impl SecretSource {
    /// Where `secret` came from, as the start line `client secret:` says it: `from
    /// SHOPIFY_API_SECRET of ./my-app/.env`, `dotenv` the `.env` of the app. lsf's own is said by
    /// its value, which is no secret, so that a webhook the backend refuses has a first place to
    /// look.
    pub fn describe(self, secret: &str, dotenv: Option<&Path>) -> String {
        match self {
            SecretSource::Explicit => "from --client-secret".to_string(),
            SecretSource::Environment => "from SHOPIFY_API_SECRET of the environment".to_string(),
            SecretSource::DotEnv => format!(
                "from SHOPIFY_API_SECRET of {}",
                dotenv.map_or_else(|| ".env".to_string(), |file| file.display().to_string())
            ),
            SecretSource::Default => format!(
                "{secret}   (lsf's own: give --client-secret, or SHOPIFY_API_SECRET in the app's .env)"
            ),
        }
    }

    /// What a webhook the backend refuses (`401`, `403`) says of its signature: the secret lsf
    /// signed it with, by where it came from ([`SecretSource::describe`]). `dotenv` is the `.env`
    /// of the app, `None` without one.
    pub fn refusal(self, secret: &str, dotenv: Option<&Path>) -> String {
        match self {
            SecretSource::Default => format!(
                "lsf signed it with {secret}, its own: give {} the backend's secret ({})",
                lsf_core::util::command(),
                match dotenv {
                    Some(_) => "--client-secret, or SHOPIFY_API_SECRET in the app's .env",
                    None => "--client-secret, or --app for the SHOPIFY_API_SECRET of its .env",
                }
            ),
            other => format!(
                "lsf signed it with the client secret {}: does the backend verify with the same one?",
                other.describe(secret, dotenv)
            ),
        }
    }
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
        // A quoted value ends at its closing quote, whatever follows it (`"s3cr3t"  # a note`);
        // an unquoted one at a `#` that starts it or follows white space.
        let quoted = ['"', '\''].into_iter().find_map(|quote| {
            let inner = value.strip_prefix(quote)?;
            inner.find(quote).map(|end| &inner[..end])
        });
        let unquoted = quoted.unwrap_or_else(|| {
            let comment = value
                .char_indices()
                .find(|&(at, c)| c == '#' && (at == 0 || value[..at].ends_with([' ', '\t'])))
                .map_or(value.len(), |(at, _)| at);
            value[..comment].trim_end()
        });
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
            "# the app\nSHOPIFY_API_KEY=abc\nexport SHOPIFY_API_SECRET=\"s3cr3t\"\n\
             QUOTED='a#b'  # a note\nNOTED=\"s3cr3t\"  # from the Dev Dashboard\n\
             PLAIN=x#y # a note\nEMPTY= # nothing\n",
        )
        .unwrap();
        assert_eq!(
            dotenv_value(&file, "SHOPIFY_API_SECRET").as_deref(),
            Some("s3cr3t")
        );
        assert_eq!(dotenv_value(&file, "NOTED").as_deref(), Some("s3cr3t"));
        assert_eq!(dotenv_value(&file, "QUOTED").as_deref(), Some("a#b"));
        assert_eq!(dotenv_value(&file, "PLAIN").as_deref(), Some("x#y"));
        assert_eq!(dotenv_value(&file, "EMPTY"), None);
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

    #[test]
    fn a_refused_webhook_names_the_secret_by_where_it_came_from() {
        let dotenv = Path::new("./my-app/.env");
        assert_eq!(
            SecretSource::DotEnv.refusal("s3cr3t", Some(dotenv)),
            "lsf signed it with the client secret from SHOPIFY_API_SECRET of ./my-app/.env: \
             does the backend verify with the same one?"
        );
        assert_eq!(
            SecretSource::Explicit.refusal("s3cr3t", None),
            "lsf signed it with the client secret from --client-secret: does the backend verify \
             with the same one?"
        );
        // lsf's own is no secret: its value, and where to give the backend's.
        assert_eq!(
            SecretSource::Default.refusal(DEFAULT_CLIENT_SECRET, Some(dotenv)),
            "lsf signed it with lsf-local-client-secret, its own: give lsf the backend's secret \
             (--client-secret, or SHOPIFY_API_SECRET in the app's .env)"
        );
        // Without an app, no `.env` is read.
        assert!(
            SecretSource::Default
                .refusal(DEFAULT_CLIENT_SECRET, None)
                .ends_with("(--client-secret, or --app for the SHOPIFY_API_SECRET of its .env)")
        );
    }
}
