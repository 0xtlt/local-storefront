//! Session tokens, which shopify.dev now calls ID tokens: the JWT App Bridge gives an embedded
//! app to prove that a request comes from a user of a shop. The app's backend verifies it with
//! its client secret, then exchanges it for an access token (token exchange). lsf mints them so
//! that a test can call the backend's authenticated routes, and verifies those it is sent back.
//!
//! The format is shopify.dev's ("ID tokens", "ID token claims"): HS256 with the client secret,
//! the claims `iss` (`https://{shop}/admin`), `dest` (`https://{shop}`), `aud` (the client id),
//! `sub` (the user id, a string), `exp`, `nbf`, `iat` (UNIX seconds), `jti` (a random UUID) and
//! `sid` (a session id, one per user and app). A token lives one minute. `@shopify/shopify-api`
//! (`decodeSessionToken`) and the `shopify_api` gem (`JwtPayload`) verify it with a 10-second
//! leeway on `exp` and `nbf`, then check `aud`; so does [`verify`].

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use super::signing::{constant_time_eq, hex, hmac_sha256, sha256};

/// How long a session token lives, in seconds: one minute, as App Bridge's.
pub const LIFETIME: i64 = 60;

/// The clock skew [`verify`] tolerates on `exp` and `nbf`, in seconds: that of Shopify's
/// libraries (`JWT_PERMITTED_CLOCK_TOLERANCE`, `JWT_LEEWAY`).
pub const LEEWAY: i64 = 10;

/// The claims of a session token, in the order Shopify writes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// The shop's admin: `https://my-shop.myshopify.com/admin`.
    pub iss: String,
    /// The shop: `https://my-shop.myshopify.com`.
    pub dest: String,
    /// The client id of the app.
    pub aud: String,
    /// The user the token was issued for: their id, as a string (`"42"`).
    pub sub: String,
    /// When the token expires.
    pub exp: i64,
    /// When the token starts being valid.
    pub nbf: i64,
    /// When the token was issued.
    pub iat: i64,
    /// A random UUID.
    pub jti: String,
    /// A session id, the same for every token of a user and an app.
    pub sid: String,
}

impl Claims {
    /// The claims of a token App Bridge would give `user` of `shop` (`my-shop.myshopify.com`)
    /// for the app `client_id`, issued at `now` (UNIX seconds). `jti` is the token's unique id:
    /// [`Claims::random_jti`] gives one.
    pub fn new(shop: &str, client_id: &str, user: u64, now: i64, jti: String) -> Claims {
        Claims {
            iss: format!("https://{shop}/admin"),
            dest: format!("https://{shop}"),
            aud: client_id.to_string(),
            sub: user.to_string(),
            exp: now + LIFETIME,
            nbf: now,
            iat: now,
            jti,
            sid: session_id(shop, client_id, user),
        }
    }

    /// A UUID (version 4 in form) made from `entropy`, for `jti`.
    pub fn uuid(entropy: &[u8]) -> String {
        let mut bytes: [u8; 16] = sha256(entropy)[..16]
            .try_into()
            .expect("a SHA-256 has 16 bytes");
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = hex(&bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }

    /// The shop the token is for: the host of `dest` (`my-shop.myshopify.com`), as Shopify's
    /// libraries read it.
    pub fn shop(&self) -> &str {
        host_of(&self.dest)
    }

    /// The user id of `sub`, when it is a number (admin tokens; a customer's token is not).
    pub fn user_id(&self) -> Option<u64> {
        self.sub.parse().ok()
    }
}

/// The session id of a user of a shop in an app: the same for each of their tokens, 64 hex
/// digits like Shopify's.
pub fn session_id(shop: &str, client_id: &str, user: u64) -> String {
    hex(&sha256(
        format!("lsf session\0{shop}\0{client_id}\0{user}").as_bytes(),
    ))
}

/// The JWT of `claims`, signed with HS256 and `secret`.
pub fn mint(claims: &Claims, secret: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap_or_default());
    let signing_input = format!("{header}.{payload}");
    let signature =
        URL_SAFE_NO_PAD.encode(hmac_sha256(secret.as_bytes(), signing_input.as_bytes()));
    format!("{signing_input}.{signature}")
}

/// Why a session token was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// Not three base64url parts, or not JSON, or a claim missing or of the wrong type.
    Malformed,
    /// A header whose `alg` is not `HS256`.
    Algorithm(String),
    /// Not signed with the client secret.
    Signature,
    /// `exp` is past, beyond the leeway.
    Expired,
    /// `nbf` is still to come, beyond the leeway.
    NotYetValid,
    /// `aud` is not the client id.
    Audience(String),
    /// The hosts of `iss` and `dest` differ.
    Issuer,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Invalid::Malformed => write!(f, "the session token is not a JWT with Shopify's claims"),
            Invalid::Algorithm(alg) => {
                write!(f, "the session token is signed with {alg}, not HS256")
            }
            Invalid::Signature => write!(
                f,
                "the session token's signature does not match the client secret"
            ),
            Invalid::Expired => write!(f, "the session token has expired (\"exp\" claim)"),
            Invalid::NotYetValid => {
                write!(f, "the session token is not valid yet (\"nbf\" claim)")
            }
            Invalid::Audience(aud) => write!(
                f,
                "Session token had invalid API key: its \"aud\" is {aud:?}, not the client id"
            ),
            Invalid::Issuer => write!(
                f,
                "the session token's \"iss\" and \"dest\" name different shops"
            ),
        }
    }
}

impl std::error::Error for Invalid {}

/// The claims of `token` when it is a session token of the app `client_id` signed with `secret`
/// and valid at `now` (UNIX seconds), with a [`LEEWAY`] on `exp` and `nbf`, as Shopify's
/// libraries check it; and the hosts of `iss` and `dest` must agree, as shopify.dev asks.
pub fn verify(token: &str, secret: &str, client_id: &str, now: i64) -> Result<Claims, Invalid> {
    let claims = verify_signature(token, secret)?;
    if claims.exp + LEEWAY < now {
        return Err(Invalid::Expired);
    }
    if claims.nbf - LEEWAY > now {
        return Err(Invalid::NotYetValid);
    }
    if claims.aud != client_id {
        return Err(Invalid::Audience(claims.aud));
    }
    if host_of(&claims.iss) != host_of(&claims.dest) {
        return Err(Invalid::Issuer);
    }
    Ok(claims)
}

/// The claims of `token` when it is an HS256 JWT signed with `secret`, whatever its dates and
/// audience.
pub fn verify_signature(token: &str, secret: &str) -> Result<Claims, Invalid> {
    let mut parts = token.trim().split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Invalid::Malformed);
    };
    let fields: serde_json::Value = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(Invalid::Malformed)?;
    match fields.get("alg").and_then(serde_json::Value::as_str) {
        Some("HS256") => {}
        Some(alg) => return Err(Invalid::Algorithm(alg.to_string())),
        None => return Err(Invalid::Malformed),
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| Invalid::Malformed)?;
    let expected = hmac_sha256(secret.as_bytes(), format!("{header}.{payload}").as_bytes());
    if !constant_time_eq(&expected, &signature) {
        return Err(Invalid::Signature);
    }
    URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(Invalid::Malformed)
}

/// The host of a URL (`https://my-shop.myshopify.com/admin` → `my-shop.myshopify.com`), or the
/// text itself when it has no scheme.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = "my-shop.myshopify.com";

    fn claims(now: i64) -> Claims {
        Claims::new(SHOP, "client-id", 42, now, Claims::uuid(b"seed"))
    }

    #[test]
    fn writes_the_claims_shopify_documents() {
        let claims = claims(1_591_764_998);
        let json = serde_json::to_value(&claims).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "iss", "dest", "aud", "sub", "exp", "nbf", "iat", "jti", "sid"
            ]
        );
        assert_eq!(json["iss"], "https://my-shop.myshopify.com/admin");
        assert_eq!(json["dest"], "https://my-shop.myshopify.com");
        assert_eq!(json["sub"], "42");
        assert_eq!(json["exp"], 1_591_765_058);
        assert_eq!(json["nbf"], 1_591_764_998);
        assert_eq!(claims.sid.len(), 64);
        assert_eq!(claims.shop(), SHOP);
        assert_eq!(claims.user_id(), Some(42));
        // A UUID of version 4, variant 1.
        let jti: Vec<&str> = claims.jti.split('-').collect();
        assert_eq!(
            jti.iter().map(|part| part.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(jti[2].starts_with('4'));
        assert!(matches!(jti[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn round_trips() {
        let claims = claims(1_000);
        let token = mint(&claims, "secret");
        let header = token.split('.').next().unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.decode(header).unwrap(),
            br#"{"alg":"HS256","typ":"JWT"}"#
        );
        assert_eq!(
            verify(&token, "secret", "client-id", 1_000),
            Ok(claims.clone())
        );
        // Within the leeway on both ends.
        assert!(verify(&token, "secret", "client-id", 1_000 + LIFETIME + LEEWAY).is_ok());
        assert!(verify(&token, "secret", "client-id", 1_000 - LEEWAY).is_ok());
    }

    #[test]
    fn refuses_expired_early_foreign_and_tampered_tokens() {
        let token = mint(&claims(1_000), "secret");
        assert_eq!(
            verify(&token, "secret", "client-id", 1_000 + LIFETIME + LEEWAY + 1),
            Err(Invalid::Expired)
        );
        assert_eq!(
            verify(&token, "secret", "client-id", 1_000 - LEEWAY - 1),
            Err(Invalid::NotYetValid)
        );
        assert_eq!(
            verify(&token, "secret", "other-client", 1_000),
            Err(Invalid::Audience("client-id".to_string()))
        );
        assert_eq!(
            verify(&token, "other", "client-id", 1_000),
            Err(Invalid::Signature)
        );
        assert_eq!(
            verify("a.b", "secret", "client-id", 1_000),
            Err(Invalid::Malformed)
        );

        // Another payload under the same signature.
        let mut other = claims(1_000);
        other.dest = "https://evil.myshopify.com".to_string();
        let forged = format!(
            "{}.{}.{}",
            token.split('.').next().unwrap(),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&other).unwrap()),
            token.split('.').nth(2).unwrap()
        );
        assert_eq!(
            verify(&forged, "secret", "client-id", 1_000),
            Err(Invalid::Signature)
        );

        // `alg: none`, the classic downgrade.
        let none = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#),
            token.split('.').nth(1).unwrap()
        );
        assert_eq!(
            verify(&none, "secret", "client-id", 1_000),
            Err(Invalid::Algorithm("none".to_string()))
        );

        // Signed, but `iss` and `dest` name two shops.
        let mut split = claims(1_000);
        split.iss = "https://other.myshopify.com/admin".to_string();
        assert_eq!(
            verify(&mint(&split, "secret"), "secret", "client-id", 1_000),
            Err(Invalid::Issuer)
        );
    }

    #[test]
    fn keeps_one_session_id_per_user_and_app() {
        assert_eq!(session_id(SHOP, "a", 1), session_id(SHOP, "a", 1));
        assert_ne!(session_id(SHOP, "a", 1), session_id(SHOP, "a", 2));
        assert_ne!(session_id(SHOP, "a", 1), session_id(SHOP, "b", 1));
    }
}
