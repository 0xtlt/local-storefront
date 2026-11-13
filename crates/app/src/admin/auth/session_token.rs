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
//!
//! Checkout and customer account UI extensions get a token of their own from
//! `shopify.sessionToken.get()`, with other claims: [`ExtensionClaims`].

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::DeserializeOwned;
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
    /// [`Claims::uuid`] makes one.
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

/// How long the session token of a checkout or customer account UI extension lives, in seconds:
/// five minutes, as shopify.dev's Session Token API says ("The token has a TTL of five minutes").
pub const EXTENSION_LIFETIME: i64 = 300;

/// The claims of the session token a checkout or customer account UI extension gets from
/// `shopify.sessionToken.get()`, which its backend verifies with the client secret
/// (`authenticate.public.checkout` and `.customerAccount` of `@shopify/shopify-app-remix` call
/// `decodeSessionToken`: HS256 with the API secret).
///
/// These are the claims of the example payload of the Session Token API of both,
/// <https://shopify.dev/docs/api/checkout-ui-extensions/latest/target-apis/platform-apis/session-token-api>
/// and
/// <https://shopify.dev/docs/api/customer-account-ui-extensions/latest/target-apis/platform-apis/session-token-api>,
/// in its order: `dest` the shop's domain, bare (`store-name.myshopify.com`, where the token of
/// the admin has `https://`), `aud`, `exp` five minutes after `iat`, `nbf`, `iat`, `jti` a UUID,
/// and `sub` the customer's GID, present only when a customer is logged in. There is no `sid`,
/// as in the example. `iss` is there only in the token of a customer account extension
/// ([`ExtensionClaims::of_customer_account`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionClaims {
    /// The issuer of a customer account extension's token: `https://my-shop.myshopify.com/checkouts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// The shop: `my-shop.myshopify.com`.
    pub dest: String,
    /// The client id of the app.
    pub aud: String,
    /// When the token expires.
    pub exp: i64,
    /// When the token starts being valid.
    pub nbf: i64,
    /// When the token was issued.
    pub iat: i64,
    /// A random UUID.
    pub jti: String,
    /// The customer logged in: `gid://shopify/Customer/42`. Absent for an anonymous buyer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
}

impl ExtensionClaims {
    /// The claims of the token an extension of the app `client_id` gets on `shop`
    /// (`my-shop.myshopify.com`) for `customer` (`None` logged out), issued at `now` (UNIX
    /// seconds). [`Claims::uuid`] makes `jti`.
    pub fn new(
        shop: &str,
        client_id: &str,
        customer: Option<u64>,
        now: i64,
        jti: String,
    ) -> ExtensionClaims {
        ExtensionClaims {
            iss: None,
            dest: host_of(shop).to_string(),
            aud: client_id.to_string(),
            exp: now + EXTENSION_LIFETIME,
            nbf: now,
            iat: now,
            jti,
            sub: customer.map(|id| format!("gid://shopify/Customer/{id}")),
        }
    }

    /// The claims of a customer account extension's token, with `iss` the shop's `/checkouts`
    /// URL first, as Shopify's tokens of these extensions have it (shopify.dev's example does not
    /// show it).
    pub fn of_customer_account(self) -> ExtensionClaims {
        ExtensionClaims {
            iss: Some(format!("https://{}/checkouts", self.dest)),
            ..self
        }
    }
}

/// The claims of `token` when it is the session token of an extension of the app `client_id`,
/// signed with `secret` and valid at `now` (UNIX seconds), with a [`LEEWAY`] on `exp` and `nbf`,
/// as Shopify's libraries check it.
pub fn verify_extension(
    token: &str,
    secret: &str,
    client_id: &str,
    now: i64,
) -> Result<ExtensionClaims, Invalid> {
    let claims: ExtensionClaims = decode(token, secret)?;
    if claims.exp + LEEWAY < now {
        return Err(Invalid::Expired);
    }
    if claims.nbf - LEEWAY > now {
        return Err(Invalid::NotYetValid);
    }
    if claims.aud != client_id {
        return Err(Invalid::Audience(claims.aud));
    }
    Ok(claims)
}

/// The session id of a user of a shop in an app: the same for each of their tokens, 64 hex
/// digits like Shopify's.
pub fn session_id(shop: &str, client_id: &str, user: u64) -> String {
    hex(&sha256(
        format!("lsf session\0{shop}\0{client_id}\0{user}").as_bytes(),
    ))
}

/// The JWT of `claims` ([`Claims`] or [`ExtensionClaims`]), signed with HS256 and `secret`.
pub fn mint(claims: &impl Serialize, secret: &str) -> String {
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
    decode(token, secret)
}

/// The payload of `token` when it is an HS256 JWT signed with `secret`.
fn decode<T: DeserializeOwned>(token: &str, secret: &str) -> Result<T, Invalid> {
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
    fn writes_the_claims_of_an_extension_as_shopify_documents() {
        let claims = ExtensionClaims::new(
            SHOP,
            "client-id",
            Some(7),
            1_679_953_753,
            Claims::uuid(b"seed"),
        );
        let json = serde_json::to_value(&claims).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        // The order of shopify.dev's example payload; no `iss`, no `sid`.
        assert_eq!(keys, ["dest", "aud", "exp", "nbf", "iat", "jti", "sub"]);
        assert_eq!(json["dest"], "my-shop.myshopify.com");
        assert_eq!(json["aud"], "client-id");
        assert_eq!(json["exp"], 1_679_954_053);
        assert_eq!(json["nbf"], 1_679_953_753);
        assert_eq!(json["iat"], 1_679_953_753);
        assert_eq!(json["sub"], "gid://shopify/Customer/7");
        // A shop given as a URL is its host.
        let of_url =
            ExtensionClaims::new("https://my-shop.myshopify.com", "c", None, 0, "j".into());
        assert_eq!(of_url.dest, SHOP);

        // Logged out: no `sub` at all.
        let anonymous = ExtensionClaims::new(SHOP, "client-id", None, 1_000, "j".into());
        let json = serde_json::to_value(&anonymous).unwrap();
        assert!(json.get("sub").is_none());

        // A customer account extension's: `iss` first, the shop's `/checkouts` URL.
        let account = ExtensionClaims::new(SHOP, "client-id", Some(7), 1_000, "j".into())
            .of_customer_account();
        let json = serde_json::to_value(&account).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["iss", "dest", "aud", "exp", "nbf", "iat", "jti", "sub"]
        );
        assert_eq!(json["iss"], "https://my-shop.myshopify.com/checkouts");
    }

    #[test]
    fn verifies_the_token_of_an_extension() {
        let claims = ExtensionClaims::new(SHOP, "client-id", Some(7), 1_000, "j".into());
        let token = mint(&claims, "secret");
        assert_eq!(
            verify_extension(&token, "secret", "client-id", 1_000),
            Ok(claims.clone())
        );
        assert!(
            verify_extension(
                &token,
                "secret",
                "client-id",
                1_000 + EXTENSION_LIFETIME + LEEWAY
            )
            .is_ok()
        );
        assert_eq!(
            verify_extension(
                &token,
                "secret",
                "client-id",
                1_000 + EXTENSION_LIFETIME + LEEWAY + 1
            ),
            Err(Invalid::Expired)
        );
        assert_eq!(
            verify_extension(&token, "other", "client-id", 1_000),
            Err(Invalid::Signature)
        );
        assert_eq!(
            verify_extension(&token, "secret", "other-client", 1_000),
            Err(Invalid::Audience("client-id".to_string()))
        );
        let anonymous = ExtensionClaims::new(SHOP, "client-id", None, 1_000, "j".into());
        assert_eq!(
            verify_extension(&mint(&anonymous, "secret"), "secret", "client-id", 1_000)
                .unwrap()
                .sub,
            None
        );
    }

    #[test]
    fn keeps_one_session_id_per_user_and_app() {
        assert_eq!(session_id(SHOP, "a", 1), session_id(SHOP, "a", 1));
        assert_ne!(session_id(SHOP, "a", 1), session_id(SHOP, "a", 2));
        assert_ne!(session_id(SHOP, "a", 1), session_id(SHOP, "b", 1));
    }
}
