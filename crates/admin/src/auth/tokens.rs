//! The access tokens of the shops: what each one grants, until when, and how it was obtained;
//! the authorization codes and refresh tokens that lead to them; the installations of the app.
//!
//! Lifetimes and rules are shopify.dev's ("Access tokens", "Token lifetimes", "Token refresh"):
//!
//! - An offline token never expires (until the app is uninstalled), unless it is an **expiring
//!   offline token** (`expiring=1`), which lives an hour and comes with a **refresh token** that
//!   lives 90 days. Refreshing returns a new pair. The refresh token presented stays usable until
//!   the app uses the new one, until a token exchange or an authorization code grant issues a new
//!   pair, 30 days after its first use, or its own expiry, whichever comes first.
//! - An online token lives 24 hours (`expires_in` 86399) and belongs to a user.
//! - A client credentials token lives 24 hours, without a refresh token.
//! - A retired access token stays valid until it expires.
//!
//! Every token is `shpat_` followed by 32 hex digits: shopify.dev says offline and online tokens
//! both begin with `shpat_` ("whichever grant produced them"); refresh tokens begin with `shprt_`.
//!
//! The registry holds no lock and reads no clock: the server keeps it behind a mutex and passes
//! the time, which is the system clock (the backend compares expiries with its own).

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::scopes::RequiredAccess;
use super::signing::{hex, hmac_sha256, sha256};

/// How long an expiring offline token lives: an hour.
pub const EXPIRING_OFFLINE_LIFETIME: i64 = 3_600;
/// How long a refresh token lives: 90 days.
pub const REFRESH_TOKEN_LIFETIME: i64 = 7_776_000;
/// How long a refresh token stays usable after its first use, when the app keeps presenting it.
pub const REFRESH_TOKEN_GRACE: i64 = 30 * 86_400;
/// How long an online token lives: 24 hours, as Shopify writes it in `expires_in`.
pub const ONLINE_LIFETIME: i64 = 86_399;
/// How long a client credentials token lives: 24 hours, as Shopify writes it in `expires_in`.
pub const CLIENT_CREDENTIALS_LIFETIME: i64 = 86_399;
/// How long an authorization code can be exchanged. Shopify does not say; ten minutes is the
/// most RFC 6749 recommends.
pub const CODE_LIFETIME: i64 = 600;
/// How long a lost migration can be recovered by sending the same request again: seven days.
pub const MIGRATION_RECOVERY: i64 = 7 * 86_400;
/// How long what can no longer be used (an expired token or code, a retired refresh token) is
/// kept before an issue forgets it ([`Tokens::purge`]): a day, so that the app presenting it
/// meanwhile is told why it is refused rather than that lsf never issued it.
pub const PURGE_AFTER: i64 = 86_400;

/// The token lsf installs the app with on `shop`: an offline token that never expires, the
/// same for a shop and an app every run, so that a test can hard-code it. `shpat_` and 32 hex
/// digits, like Shopify's.
pub fn default_token(shop: &str, client_id: &str) -> String {
    format!(
        "shpat_{}",
        &hex(&sha256(
            format!("lsf default token\0{shop}\0{client_id}").as_bytes()
        ))[..32]
    )
}

/// Whether a token acts for the app (offline) or for a user of the shop (online).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    Offline,
    Online,
}

/// How a token was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantSource {
    /// Installed by lsf: [`default_token`].
    Install,
    /// `POST /admin/oauth/access_token` with a `code`.
    AuthorizationCode,
    /// Token exchange of a session token.
    TokenExchange,
    /// Token exchange of a non-expiring offline token for an expiring one.
    Migration,
    /// `grant_type=refresh_token`.
    RefreshToken,
    /// `grant_type=client_credentials`.
    ClientCredentials,
    /// `delegateAccessTokenCreate`: a subset of its parent's scopes, never more than the
    /// installation grants.
    Delegate,
}

/// The staff member an online token acts for: `associated_user` of the token response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssociatedUser {
    pub id: u64,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub email_verified: bool,
    pub account_owner: bool,
    pub locale: String,
    pub collaborator: bool,
}

impl AssociatedUser {
    /// lsf's staff member `id` of `shop`, the account owner when `id` is 1: what the shop's
    /// users are when the store data says nothing of them.
    pub fn local(shop: &str, id: u64) -> AssociatedUser {
        let owner = id == 1;
        AssociatedUser {
            id,
            first_name: if owner { "Shop" } else { "Staff" }.to_string(),
            last_name: if owner {
                "Owner".to_string()
            } else {
                id.to_string()
            },
            email: format!(
                "{}@{shop}",
                if owner {
                    "owner".to_string()
                } else {
                    format!("staff-{id}")
                }
            ),
            email_verified: true,
            account_owner: owner,
            locale: "en".to_string(),
            collaborator: false,
        }
    }
}

/// What a token grants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// The shop: `my-shop.myshopify.com`.
    pub shop: String,
    /// The app.
    pub client_id: String,
    /// The scopes the token carries, as granted.
    pub scopes: Vec<String>,
    pub mode: AccessMode,
    /// The user of an online token.
    pub user: Option<AssociatedUser>,
    pub issued_at: DateTime<Utc>,
    /// `None` for a token that does not expire.
    pub expires_at: Option<DateTime<Utc>>,
    pub source: GrantSource,
}

impl Grant {
    /// Whether the token has expired at `now`.
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }

    /// Whether the token's scopes grant `access`.
    pub fn allows(&self, access: &RequiredAccess) -> bool {
        access.allows(&self.scopes)
    }

    /// Whether the token's scopes cover `scope`.
    pub fn covers(&self, scope: &str) -> bool {
        super::scopes::covers(&self.scopes, scope)
    }
}

/// Why a token is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenError {
    /// Never issued, revoked, or migrated.
    Unknown,
    /// Expired.
    Expired,
}

/// A token just issued, and the JSON the token endpoint answers with.
#[derive(Clone, Debug, PartialEq)]
pub struct Issued {
    pub access_token: String,
    pub grant: Grant,
    /// The refresh token of an expiring offline token.
    pub refresh_token: Option<String>,
    pub refresh_token_expires_at: Option<DateTime<Utc>>,
}

impl Issued {
    /// The body of Shopify's answer: `access_token` and `scope`, then `expires_in`,
    /// `associated_user_scope` and `associated_user` for an online token, `expires_in`,
    /// `refresh_token` and `refresh_token_expires_in` for an expiring offline one, `expires_in`
    /// for a client credentials one. The fields `@shopify/shopify-api` (`createSession`) and the
    /// `shopify_api` gem (`AccessTokenResponse`) read.
    pub fn response(&self, now: DateTime<Utc>) -> Value {
        let mut body = Map::new();
        body.insert("access_token".into(), json!(self.access_token));
        body.insert("scope".into(), json!(self.grant.scopes.join(",")));
        if let Some(expires_at) = self.grant.expires_at {
            body.insert("expires_in".into(), json!(seconds_until(expires_at, now)));
        }
        if let Some(user) = &self.grant.user {
            body.insert(
                "associated_user_scope".into(),
                json!(self.grant.scopes.join(",")),
            );
            body.insert("associated_user".into(), json!(user));
        }
        if let (Some(token), Some(expires_at)) =
            (&self.refresh_token, self.refresh_token_expires_at)
        {
            body.insert("refresh_token".into(), json!(token));
            body.insert(
                "refresh_token_expires_in".into(),
                json!(seconds_until(expires_at, now)),
            );
        }
        Value::Object(body)
    }
}

fn seconds_until(at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    (at - now).num_seconds().max(0)
}

/// An authorization code `GET /admin/oauth/authorize` gave, until it is exchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    pub shop: String,
    pub client_id: String,
    /// The scopes approved.
    pub scopes: Vec<String>,
    /// `Online` for `grant_options[]=per-user`.
    pub mode: AccessMode,
    pub user: Option<AssociatedUser>,
    pub redirect_uri: String,
    pub expires_at: DateTime<Utc>,
}

/// Why a refresh is refused: always Shopify's `401` `invalid_request`, "This request requires an
/// active refresh_token". The variant says why, for lsf's own messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshError {
    Unknown,
    /// Issued to another app.
    OtherClient,
    /// Replaced by one the app has used since, or by a newer token acquisition.
    Retired,
    /// Past its 90 days, or 30 days after its first use.
    Expired,
}

/// Why a migration of a non-expiring offline token is refused (`invalid_subject_token`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// Unknown, revoked, of another app or shop.
    Unknown,
    /// An online token, or one that already expires.
    NotMigratable,
    /// A delegate token: it can make no token ("A delegate access token can't create more
    /// delegate access tokens", shopify.dev, "Delegate API access"), and an expiring offline
    /// token made from it would carry every scope of the installation, and could.
    Delegate,
}

#[derive(Clone, Debug)]
struct RefreshEntry {
    shop: String,
    client_id: String,
    scopes: Vec<String>,
    expires_at: DateTime<Utc>,
    first_used_at: Option<DateTime<Utc>>,
    /// The refresh token this one replaced: using this one retires it.
    predecessor: Option<String>,
    /// When it was retired: replaced by a newer one the app used, or by a new grant.
    retired: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
struct Migration {
    access_token: String,
    refresh_token: String,
    at: DateTime<Utc>,
}

/// The tokens of every shop, the codes and refresh tokens, and the installations.
#[derive(Clone, Debug)]
pub struct Tokens {
    seed: [u8; 32],
    counter: u64,
    access: HashMap<String, Grant>,
    refresh: HashMap<String, RefreshEntry>,
    codes: HashMap<String, Code>,
    /// The scopes the app is installed with, by shop and client id.
    installs: BTreeMap<(String, String), Vec<String>>,
    /// Migrated non-expiring tokens, for a lost answer to be sent again.
    migrations: HashMap<String, Migration>,
}

impl Default for Tokens {
    fn default() -> Self {
        Tokens::new()
    }
}

impl Tokens {
    /// An empty registry whose tokens cannot be guessed from one run to the next.
    pub fn new() -> Tokens {
        let mut entropy = Vec::new();
        for _ in 0..4 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_nanos()),
            );
            entropy.extend_from_slice(&hasher.finish().to_le_bytes());
        }
        entropy.extend_from_slice(&std::process::id().to_le_bytes());
        Tokens::from_seed(sha256(&entropy))
    }

    /// An empty registry whose tokens are the same every run with the same `seed`, for tests.
    pub fn with_seed(seed: u64) -> Tokens {
        Tokens::from_seed(sha256(&seed.to_le_bytes()))
    }

    fn from_seed(seed: [u8; 32]) -> Tokens {
        Tokens {
            seed,
            counter: 0,
            access: HashMap::new(),
            refresh: HashMap::new(),
            codes: HashMap::new(),
            installs: BTreeMap::new(),
            migrations: HashMap::new(),
        }
    }

    /// 32 bytes no one can predict without the seed, different at each call.
    pub fn entropy(&mut self, purpose: &str) -> [u8; 32] {
        self.counter += 1;
        hmac_sha256(
            &self.seed,
            format!("{purpose}\0{}", self.counter).as_bytes(),
        )
    }

    /// A new secret value: `prefix` and 32 hex digits.
    fn fresh(&mut self, prefix: &str) -> String {
        format!("{prefix}{}", &hex(&self.entropy(prefix))[..32])
    }

    /// Installs the app `client_id` on `shop` with `scopes`, or changes the scopes it is
    /// installed with, and answers its [`default_token`].
    pub fn install(
        &mut self,
        shop: &str,
        client_id: &str,
        scopes: &[String],
        now: DateTime<Utc>,
    ) -> String {
        self.installs
            .insert((shop.to_string(), client_id.to_string()), scopes.to_vec());
        // Scopes are approved (and revoked) for an installation, not for a token (shopify.dev,
        // "Manage access scopes" and `appRevokeAccessScopes`): the tokens the installation
        // issued, by token exchange or OAuth, carry its new scopes from now on.
        self.set_scopes(shop, client_id, scopes);
        let token = default_token(shop, client_id);
        self.access.insert(
            token.clone(),
            Grant {
                shop: shop.to_string(),
                client_id: client_id.to_string(),
                scopes: scopes.to_vec(),
                mode: AccessMode::Offline,
                user: None,
                issued_at: now,
                expires_at: None,
                source: GrantSource::Install,
            },
        );
        token
    }

    /// Changes the scopes `client_id` is installed with on `shop`, and those of the tokens of the
    /// installation: each carries the new ones, but a delegate token, which keeps those of its
    /// own the installation still grants ("If your app is later re-authorized with fewer
    /// scopes, every existing delegate token loses the scopes that are no longer authorized",
    /// shopify.dev, "Delegate API access"). Nothing for an app not installed.
    pub fn set_scopes(&mut self, shop: &str, client_id: &str, scopes: &[String]) {
        let key = (shop.to_string(), client_id.to_string());
        if let Some(installed) = self.installs.get_mut(&key) {
            *installed = scopes.to_vec();
        }
        let installation =
            |grant_shop: &str, grant_client: &str| grant_shop == shop && grant_client == client_id;
        for grant in self.access.values_mut() {
            if !installation(&grant.shop, &grant.client_id) {
                continue;
            }
            if grant.source == GrantSource::Delegate {
                grant
                    .scopes
                    .retain(|scope| super::scopes::covers(scopes, scope));
            } else {
                grant.scopes = scopes.to_vec();
            }
        }
        for entry in self.refresh.values_mut() {
            if installation(&entry.shop, &entry.client_id) {
                entry.scopes = scopes.to_vec();
            }
        }
    }

    /// Keeps a delegate token of the app `client_id` on `shop`: `delegateAccessTokenCreate`
    /// made it, with these scopes and this expiry (its parent's, or sooner).
    pub fn delegate(
        &mut self,
        token: &str,
        shop: &str,
        client_id: &str,
        scopes: &[String],
        expires_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) {
        self.purge(now - Duration::seconds(PURGE_AFTER));
        self.access.insert(
            token.to_string(),
            Grant {
                shop: shop.to_string(),
                client_id: client_id.to_string(),
                scopes: scopes.to_vec(),
                mode: AccessMode::Offline,
                user: None,
                issued_at: now,
                expires_at,
                source: GrantSource::Delegate,
            },
        );
    }

    /// The scopes `client_id` is installed with on `shop`, if it is installed.
    pub fn installed_scopes(&self, shop: &str, client_id: &str) -> Option<&[String]> {
        self.installs
            .get(&(shop.to_string(), client_id.to_string()))
            .map(Vec::as_slice)
    }

    /// Uninstalls `client_id` from `shop`: its tokens, refresh tokens and codes stop working.
    /// What `app/uninstalled` reports. Answers the number of access tokens revoked.
    pub fn uninstall(&mut self, shop: &str, client_id: &str) -> usize {
        self.installs
            .remove(&(shop.to_string(), client_id.to_string()));
        let before = self.access.len();
        self.access
            .retain(|_, grant| !(grant.shop == shop && grant.client_id == client_id));
        self.refresh
            .retain(|_, entry| !(entry.shop == shop && entry.client_id == client_id));
        self.codes
            .retain(|_, code| !(code.shop == shop && code.client_id == client_id));
        before - self.access.len()
    }

    /// Revokes every token of every app on `shop`, and its installations: a shop reset or
    /// deleted. Answers the number of access tokens revoked.
    pub fn revoke_shop(&mut self, shop: &str) -> usize {
        self.installs.retain(|(installed, _), _| installed != shop);
        let before = self.access.len();
        self.access.retain(|_, grant| grant.shop != shop);
        self.refresh.retain(|_, entry| entry.shop != shop);
        self.codes.retain(|_, code| code.shop != shop);
        before - self.access.len()
    }

    /// Revokes one access token. Whether it existed.
    pub fn revoke(&mut self, token: &str) -> bool {
        self.access.remove(token).is_some()
    }

    /// What `token` grants at `now`: the check of `X-Shopify-Access-Token`.
    pub fn lookup(&self, token: &str, now: DateTime<Utc>) -> Result<&Grant, TokenError> {
        let grant = self.access.get(token.trim()).ok_or(TokenError::Unknown)?;
        if grant.expired(now) {
            return Err(TokenError::Expired);
        }
        Ok(grant)
    }

    /// The tokens issued, valid or expired.
    pub fn grants(&self) -> impl Iterator<Item = (&str, &Grant)> {
        self.access
            .iter()
            .map(|(token, grant)| (token.as_str(), grant))
    }

    /// Forgets the tokens, codes and refresh tokens that can no longer be used at `now`. Each
    /// issue calls it for [`PURGE_AFTER`] ago, so that a long run does not keep them all.
    pub fn purge(&mut self, now: DateTime<Utc>) {
        self.access.retain(|_, grant| !grant.expired(now));
        self.codes.retain(|_, code| code.expires_at > now);
        self.refresh.retain(|_, entry| {
            entry.retired.is_none_or(|retired| retired > now) && refresh_usable_until(entry) > now
        });
        self.migrations
            .retain(|_, migration| migration.at + Duration::seconds(MIGRATION_RECOVERY) > now);
    }

    /// Issues an authorization code for `code`'s grant, which [`Tokens::redeem_code`] exchanges
    /// once.
    pub fn create_code(&mut self, code: Code) -> String {
        let value = hex(&self.entropy("code"))[..32].to_string();
        self.codes.insert(value.clone(), code);
        value
    }

    /// The shop of an authorization code, without using it.
    pub fn code_shop(&self, code: &str) -> Option<&str> {
        self.codes.get(code).map(|code| code.shop.as_str())
    }

    /// The shop of a refresh token, without using it.
    pub fn refresh_token_shop(&self, refresh_token: &str) -> Option<&str> {
        self.refresh
            .get(refresh_token)
            .map(|entry| entry.shop.as_str())
    }

    /// The grant of an authorization code of `client_id`, which can no longer be used after
    /// this call. `None` when it is unknown, used, expired or another app's.
    pub fn redeem_code(&mut self, code: &str, client_id: &str, now: DateTime<Utc>) -> Option<Code> {
        let entry = self.codes.get(code)?;
        if entry.client_id != client_id {
            return None;
        }
        let entry = self.codes.remove(code)?;
        (entry.expires_at > now).then_some(entry)
    }

    /// Issues an offline token, expiring (with a refresh token) or not. An expiring one
    /// acquired by a grant (not a refresh) retires the other refresh tokens of the app on the
    /// shop.
    pub fn issue_offline(
        &mut self,
        shop: &str,
        client_id: &str,
        scopes: &[String],
        expiring: bool,
        source: GrantSource,
        now: DateTime<Utc>,
    ) -> Issued {
        if !expiring {
            return self.issue(
                Grant {
                    shop: shop.to_string(),
                    client_id: client_id.to_string(),
                    scopes: scopes.to_vec(),
                    mode: AccessMode::Offline,
                    user: None,
                    issued_at: now,
                    expires_at: None,
                    source,
                },
                None,
            );
        }
        if source != GrantSource::RefreshToken {
            for entry in self.refresh.values_mut() {
                if entry.shop == shop && entry.client_id == client_id {
                    entry.retired.get_or_insert(now);
                }
            }
        }
        self.issue_expiring(shop, client_id, scopes, source, None, now)
    }

    fn issue_expiring(
        &mut self,
        shop: &str,
        client_id: &str,
        scopes: &[String],
        source: GrantSource,
        predecessor: Option<String>,
        now: DateTime<Utc>,
    ) -> Issued {
        let refresh_token = self.fresh("shprt_");
        let refresh_expires_at = now + Duration::seconds(REFRESH_TOKEN_LIFETIME);
        self.refresh.insert(
            refresh_token.clone(),
            RefreshEntry {
                shop: shop.to_string(),
                client_id: client_id.to_string(),
                scopes: scopes.to_vec(),
                expires_at: refresh_expires_at,
                first_used_at: None,
                predecessor,
                retired: None,
            },
        );
        self.issue(
            Grant {
                shop: shop.to_string(),
                client_id: client_id.to_string(),
                scopes: scopes.to_vec(),
                mode: AccessMode::Offline,
                user: None,
                issued_at: now,
                expires_at: Some(now + Duration::seconds(EXPIRING_OFFLINE_LIFETIME)),
                source,
            },
            Some((refresh_token, refresh_expires_at)),
        )
    }

    /// Issues an online token for `user`, which lives [`ONLINE_LIFETIME`].
    pub fn issue_online(
        &mut self,
        shop: &str,
        client_id: &str,
        scopes: &[String],
        user: AssociatedUser,
        source: GrantSource,
        now: DateTime<Utc>,
    ) -> Issued {
        self.issue(
            Grant {
                shop: shop.to_string(),
                client_id: client_id.to_string(),
                scopes: scopes.to_vec(),
                mode: AccessMode::Online,
                user: Some(user),
                issued_at: now,
                expires_at: Some(now + Duration::seconds(ONLINE_LIFETIME)),
                source,
            },
            None,
        )
    }

    /// Issues a client credentials token, which lives [`CLIENT_CREDENTIALS_LIFETIME`].
    pub fn issue_client_credentials(
        &mut self,
        shop: &str,
        client_id: &str,
        scopes: &[String],
        now: DateTime<Utc>,
    ) -> Issued {
        self.issue(
            Grant {
                shop: shop.to_string(),
                client_id: client_id.to_string(),
                scopes: scopes.to_vec(),
                mode: AccessMode::Offline,
                user: None,
                issued_at: now,
                expires_at: Some(now + Duration::seconds(CLIENT_CREDENTIALS_LIFETIME)),
                source: GrantSource::ClientCredentials,
            },
            None,
        )
    }

    fn issue(&mut self, grant: Grant, refresh: Option<(String, DateTime<Utc>)>) -> Issued {
        self.purge(grant.issued_at - Duration::seconds(PURGE_AFTER));
        let access_token = self.fresh("shpat_");
        self.access.insert(access_token.clone(), grant.clone());
        let (refresh_token, refresh_token_expires_at) = refresh.unzip();
        Issued {
            access_token,
            grant,
            refresh_token,
            refresh_token_expires_at,
        }
    }

    /// A new pair for a refresh token of `client_id`, with the rotation rules of the module.
    pub fn refresh(
        &mut self,
        refresh_token: &str,
        client_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Issued, RefreshError> {
        let entry = self
            .refresh
            .get(refresh_token)
            .ok_or(RefreshError::Unknown)?;
        if entry.client_id != client_id {
            return Err(RefreshError::OtherClient);
        }
        if entry.retired.is_some() {
            return Err(RefreshError::Retired);
        }
        if refresh_usable_until(entry) <= now {
            return Err(RefreshError::Expired);
        }
        let (shop, scopes, predecessor) = (
            entry.shop.clone(),
            entry.scopes.clone(),
            entry.predecessor.clone(),
        );
        // Using this token proves the app received it: the one it replaced is done.
        if let Some(predecessor) = predecessor
            && let Some(previous) = self.refresh.get_mut(&predecessor)
        {
            previous.retired.get_or_insert(now);
        }
        if let Some(entry) = self.refresh.get_mut(refresh_token) {
            entry.first_used_at.get_or_insert(now);
        }
        Ok(self.issue_expiring(
            &shop,
            client_id,
            &scopes,
            GrantSource::RefreshToken,
            Some(refresh_token.to_string()),
            now,
        ))
    }

    /// Exchanges a non-expiring offline token of `client_id` on `shop` for an expiring one and
    /// its refresh token; the old token stops working. A delegate token is refused: it is not
    /// the app's offline token, and what it would be exchanged for is. Sending the same request again within
    /// [`MIGRATION_RECOVERY`] answers the same pair, as long as it was not refreshed or retired.
    pub fn migrate(
        &mut self,
        token: &str,
        client_id: &str,
        shop: &str,
        now: DateTime<Utc>,
    ) -> Result<Issued, MigrationError> {
        if let Some(migration) = self.migrations.get(token).cloned()
            && migration.at + Duration::seconds(MIGRATION_RECOVERY) > now
            && let Some(entry) = self.refresh.get(&migration.refresh_token)
            && entry.retired.is_none()
            && entry.first_used_at.is_none()
            && entry.shop == shop
            && entry.client_id == client_id
            && let Some(grant) = self.access.get_mut(&migration.access_token)
        {
            // Shopify extends the access token when needed, not the refresh token.
            let lifetime = now + Duration::seconds(EXPIRING_OFFLINE_LIFETIME);
            if grant.expires_at.is_none_or(|at| at < lifetime) {
                grant.expires_at = Some(lifetime);
            }
            return Ok(Issued {
                access_token: migration.access_token.clone(),
                grant: grant.clone(),
                refresh_token: Some(migration.refresh_token.clone()),
                refresh_token_expires_at: Some(entry.expires_at),
            });
        }
        let grant = self.access.get(token).ok_or(MigrationError::Unknown)?;
        if grant.client_id != client_id || grant.shop != shop {
            return Err(MigrationError::Unknown);
        }
        if grant.source == GrantSource::Delegate {
            return Err(MigrationError::Delegate);
        }
        if grant.mode != AccessMode::Offline || grant.expires_at.is_some() {
            return Err(MigrationError::NotMigratable);
        }
        let scopes = grant.scopes.clone();
        self.access.remove(token);
        let issued =
            self.issue_offline(shop, client_id, &scopes, true, GrantSource::Migration, now);
        self.migrations.insert(
            token.to_string(),
            Migration {
                access_token: issued.access_token.clone(),
                refresh_token: issued.refresh_token.clone().unwrap_or_default(),
                at: now,
            },
        );
        Ok(issued)
    }
}

/// When a refresh token stops being usable: its expiry, or 30 days after its first use.
fn refresh_usable_until(entry: &RefreshEntry) -> DateTime<Utc> {
    match entry.first_used_at {
        Some(first) => entry
            .expires_at
            .min(first + Duration::seconds(REFRESH_TOKEN_GRACE)),
        None => entry.expires_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = "my-shop.myshopify.com";

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn scopes() -> Vec<String> {
        vec!["write_products".to_string(), "read_orders".to_string()]
    }

    #[test]
    fn installs_with_a_deterministic_token() {
        let token = default_token(SHOP, "client");
        assert!(token.starts_with("shpat_"));
        assert_eq!(token.len(), 6 + 32);
        assert_eq!(token, default_token(SHOP, "client"));
        assert_ne!(token, default_token("other.myshopify.com", "client"));
        assert_ne!(token, default_token(SHOP, "other"));

        let mut tokens = Tokens::with_seed(1);
        assert_eq!(tokens.install(SHOP, "client", &scopes(), at(0)), token);
        let grant = tokens.lookup(&token, at(1_000_000)).unwrap();
        assert_eq!(grant.mode, AccessMode::Offline);
        assert_eq!(grant.expires_at, None);
        assert!(grant.covers("read_products"));
        assert_eq!(tokens.installed_scopes(SHOP, "client"), Some(&scopes()[..]));
        assert_eq!(tokens.lookup("shpat_nope", at(0)), Err(TokenError::Unknown));
    }

    #[test]
    fn installing_again_changes_the_scopes_of_every_token_of_the_app() {
        let mut tokens = Tokens::with_seed(1);
        let default = tokens.install(SHOP, "client", &scopes(), at(0));
        let exchanged = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(1),
        );
        let other_app = tokens.issue_offline(
            SHOP,
            "other",
            &scopes(),
            false,
            GrantSource::TokenExchange,
            at(1),
        );
        let fewer = vec!["read_products".to_string()];
        tokens.install(SHOP, "client", &fewer, at(2));
        for token in [&default, &exchanged.access_token] {
            let grant = tokens.lookup(token, at(3)).unwrap();
            assert_eq!(grant.scopes, fewer, "{token}");
            assert!(!grant.covers("write_products"));
        }
        // The refresh token gives the installation's scopes too.
        let refreshed = tokens
            .refresh(exchanged.refresh_token.as_deref().unwrap(), "client", at(4))
            .unwrap();
        assert_eq!(refreshed.grant.scopes, fewer);
        // Another app's tokens keep theirs.
        let grant = tokens.lookup(&other_app.access_token, at(3)).unwrap();
        assert_eq!(grant.scopes, scopes());
    }

    #[test]
    fn expires_online_and_client_credentials_tokens() {
        let mut tokens = Tokens::with_seed(1);
        let online = tokens.issue_online(
            SHOP,
            "client",
            &scopes(),
            AssociatedUser::local(SHOP, 1),
            GrantSource::TokenExchange,
            at(0),
        );
        assert!(
            tokens
                .lookup(&online.access_token, at(ONLINE_LIFETIME - 1))
                .is_ok()
        );
        assert_eq!(
            tokens.lookup(&online.access_token, at(ONLINE_LIFETIME)),
            Err(TokenError::Expired)
        );
        let body = online.response(at(0));
        assert_eq!(body["expires_in"], 86_399);
        assert_eq!(body["associated_user_scope"], "write_products,read_orders");
        assert_eq!(body["associated_user"]["account_owner"], true);
        assert_eq!(
            body["associated_user"]["email"],
            "owner@my-shop.myshopify.com"
        );

        let credentials = tokens.issue_client_credentials(SHOP, "client", &scopes(), at(0));
        assert_eq!(
            credentials.response(at(0)),
            json!({
                "access_token": credentials.access_token,
                "scope": "write_products,read_orders",
                "expires_in": 86_399,
            })
        );
        tokens.purge(at(CLIENT_CREDENTIALS_LIFETIME));
        assert_eq!(
            tokens.lookup(&credentials.access_token, at(0)),
            Err(TokenError::Unknown)
        );
    }

    /// An issue forgets what could no longer be used a day ago: until then, a token presented
    /// is refused for what it is (expired, retired), and what is still valid always stays.
    #[test]
    fn issuing_forgets_what_expired_a_day_ago() {
        let mut tokens = Tokens::with_seed(1);
        let installed = tokens.install(SHOP, "client", &scopes(), at(0));
        let online = tokens.issue_online(
            SHOP,
            "client",
            &scopes(),
            AssociatedUser::local(SHOP, 1),
            GrantSource::TokenExchange,
            at(0),
        );
        let exchange = |tokens: &mut Tokens, now| {
            tokens.issue_offline(
                SHOP,
                "client",
                &scopes(),
                true,
                GrantSource::TokenExchange,
                now,
            )
        };
        let first = exchange(&mut tokens, at(0));
        let r1 = first.refresh_token.clone().unwrap();
        // The online token expires as a new grant retires r1.
        let r2 = exchange(&mut tokens, at(ONLINE_LIFETIME))
            .refresh_token
            .unwrap();
        let later = ONLINE_LIFETIME + 3_600;
        let code = tokens.create_code(Code {
            shop: SHOP.to_string(),
            client_id: "client".to_string(),
            scopes: scopes(),
            mode: AccessMode::Offline,
            user: None,
            redirect_uri: "https://app.example/callback".to_string(),
            expires_at: at(later - 60),
        });

        // Within the day after: still refused for what they are.
        tokens.issue_client_credentials(SHOP, "client", &scopes(), at(later));
        assert_eq!(
            tokens.lookup(&online.access_token, at(later)),
            Err(TokenError::Expired)
        );
        assert_eq!(
            tokens.refresh(&r1, "client", at(later)),
            Err(RefreshError::Retired)
        );
        assert!(tokens.code_shop(&code).is_some());

        // A day after they could no longer be used: forgotten. What is valid stays.
        let much_later = later + PURGE_AFTER;
        tokens.issue_client_credentials(SHOP, "client", &scopes(), at(much_later));
        for token in [&online.access_token, &first.access_token] {
            assert_eq!(
                tokens.lookup(token, at(much_later)),
                Err(TokenError::Unknown)
            );
        }
        assert_eq!(
            tokens.refresh(&r1, "client", at(much_later)),
            Err(RefreshError::Unknown)
        );
        assert!(tokens.code_shop(&code).is_none());
        assert!(tokens.lookup(&installed, at(much_later)).is_ok());
        assert!(tokens.refresh(&r2, "client", at(much_later)).is_ok());
    }

    #[test]
    fn rotates_refresh_tokens_as_shopify() {
        let mut tokens = Tokens::with_seed(1);
        let first = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(0),
        );
        let body = first.response(at(0));
        assert_eq!(body["expires_in"], 3_600);
        assert_eq!(body["refresh_token_expires_in"], 7_776_000);
        let r1 = first.refresh_token.clone().unwrap();
        assert!(r1.starts_with("shprt_"));
        assert_eq!(
            tokens.lookup(&first.access_token, at(3_600)),
            Err(TokenError::Expired)
        );

        // Another app cannot use it.
        assert_eq!(
            tokens.refresh(&r1, "other", at(10)),
            Err(RefreshError::OtherClient)
        );
        // A lost answer: r1 still works until r2 is used.
        let second = tokens.refresh(&r1, "client", at(3_000)).unwrap();
        let again = tokens.refresh(&r1, "client", at(3_100)).unwrap();
        assert_eq!(again.grant.source, GrantSource::RefreshToken);
        assert_ne!(second.access_token, again.access_token);
        // The old access token stays valid until it expires.
        assert!(tokens.lookup(&first.access_token, at(3_100)).is_ok());
        // Using r3 (the newest) retires r1.
        let r3 = again.refresh_token.clone().unwrap();
        tokens.refresh(&r3, "client", at(4_000)).unwrap();
        assert_eq!(
            tokens.refresh(&r1, "client", at(4_001)),
            Err(RefreshError::Retired)
        );
        // 30 days after its first use, a refresh token is done.
        let r2 = second.refresh_token.clone().unwrap();
        tokens.refresh(&r2, "client", at(5_000)).unwrap();
        assert_eq!(
            tokens.refresh(&r2, "client", at(5_000 + REFRESH_TOKEN_GRACE)),
            Err(RefreshError::Expired)
        );
        assert_eq!(
            tokens.refresh("shprt_unknown", "client", at(0)),
            Err(RefreshError::Unknown)
        );
    }

    #[test]
    fn a_new_grant_retires_every_refresh_token() {
        let mut tokens = Tokens::with_seed(1);
        let first = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(0),
        );
        tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::AuthorizationCode,
            at(10),
        );
        assert_eq!(
            tokens.refresh(first.refresh_token.as_deref().unwrap(), "client", at(20)),
            Err(RefreshError::Retired)
        );
        // Unless it is past 90 days, a refresh token never used is still good.
        let fresh = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(30),
        );
        let token = fresh.refresh_token.unwrap();
        assert_eq!(
            tokens.refresh(&token, "client", at(30 + REFRESH_TOKEN_LIFETIME)),
            Err(RefreshError::Expired)
        );
        assert!(
            tokens
                .refresh(&token, "client", at(29 + REFRESH_TOKEN_LIFETIME))
                .is_ok()
        );
    }

    #[test]
    fn migrates_a_non_expiring_token_once() {
        let mut tokens = Tokens::with_seed(1);
        let old = tokens.install(SHOP, "client", &scopes(), at(0));
        assert_eq!(
            tokens.migrate(&old, "other", SHOP, at(1)),
            Err(MigrationError::Unknown)
        );
        let migrated = tokens.migrate(&old, "client", SHOP, at(1)).unwrap();
        assert_eq!(migrated.grant.source, GrantSource::Migration);
        assert_eq!(tokens.lookup(&old, at(2)), Err(TokenError::Unknown));
        // The same request again answers the same pair, the access token extended.
        let recovered = tokens.migrate(&old, "client", SHOP, at(7_200)).unwrap();
        assert_eq!(recovered.access_token, migrated.access_token);
        assert_eq!(recovered.refresh_token, migrated.refresh_token);
        assert!(tokens.lookup(&migrated.access_token, at(7_300)).is_ok());
        // Once the pair is refreshed, the old token is gone for good.
        tokens
            .refresh(
                migrated.refresh_token.as_deref().unwrap(),
                "client",
                at(7_400),
            )
            .unwrap();
        assert_eq!(
            tokens.migrate(&old, "client", SHOP, at(7_500)),
            Err(MigrationError::Unknown)
        );
        // An expiring token cannot be migrated.
        let expiring = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(0),
        );
        assert_eq!(
            tokens.migrate(&expiring.access_token, "client", SHOP, at(1)),
            Err(MigrationError::NotMigratable)
        );
    }

    #[test]
    fn a_delegate_token_cannot_be_migrated_to_a_token_of_the_installation() {
        let mut tokens = Tokens::with_seed(1);
        tokens.install(SHOP, "client", &scopes(), at(0));
        let read_only = vec!["read_products".to_string()];
        tokens.delegate("shppa_1", SHOP, "client", &read_only, None, at(0));
        assert_eq!(
            tokens.migrate("shppa_1", "client", SHOP, at(1)),
            Err(MigrationError::Delegate)
        );
        // It still works, with its own scopes, and an install with more scopes leaves them.
        assert_eq!(tokens.lookup("shppa_1", at(2)).unwrap().scopes, read_only);
        tokens.install(SHOP, "client", &scopes(), at(3));
        assert_eq!(tokens.lookup("shppa_1", at(4)).unwrap().scopes, read_only);
    }

    #[test]
    fn redeems_a_code_once() {
        let mut tokens = Tokens::with_seed(1);
        let code = tokens.create_code(Code {
            shop: SHOP.to_string(),
            client_id: "client".to_string(),
            scopes: scopes(),
            mode: AccessMode::Offline,
            user: None,
            redirect_uri: "https://app.example/callback".to_string(),
            expires_at: at(CODE_LIFETIME),
        });
        assert_eq!(code.len(), 32);
        assert_eq!(tokens.redeem_code(&code, "other", at(0)), None);
        assert!(tokens.redeem_code(&code, "client", at(0)).is_some());
        assert_eq!(tokens.redeem_code(&code, "client", at(0)), None);
        let late = tokens.create_code(Code {
            shop: SHOP.to_string(),
            client_id: "client".to_string(),
            scopes: scopes(),
            mode: AccessMode::Offline,
            user: None,
            redirect_uri: String::new(),
            expires_at: at(CODE_LIFETIME),
        });
        assert_eq!(tokens.redeem_code(&late, "client", at(CODE_LIFETIME)), None);
    }

    #[test]
    fn uninstalling_revokes_every_token_of_the_app() {
        let mut tokens = Tokens::with_seed(1);
        let default = tokens.install(SHOP, "client", &scopes(), at(0));
        let issued = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(0),
        );
        let other_shop = tokens.install("other.myshopify.com", "client", &scopes(), at(0));
        assert_eq!(tokens.uninstall(SHOP, "client"), 2);
        assert_eq!(tokens.lookup(&default, at(1)), Err(TokenError::Unknown));
        assert_eq!(
            tokens.refresh(issued.refresh_token.as_deref().unwrap(), "client", at(1)),
            Err(RefreshError::Unknown)
        );
        assert_eq!(tokens.installed_scopes(SHOP, "client"), None);
        assert!(tokens.lookup(&other_shop, at(1)).is_ok());
        assert_eq!(tokens.revoke_shop("other.myshopify.com"), 1);
        assert!(tokens.grants().next().is_none());
    }

    #[test]
    fn issues_different_tokens_from_one_seed_to_another() {
        let mut a = Tokens::with_seed(1);
        let mut b = Tokens::with_seed(1);
        let mut c = Tokens::with_seed(2);
        let issue = |tokens: &mut Tokens| {
            tokens
                .issue_offline(
                    SHOP,
                    "client",
                    &scopes(),
                    false,
                    GrantSource::AuthorizationCode,
                    at(0),
                )
                .access_token
        };
        let (from_a, from_b, from_c) = (issue(&mut a), issue(&mut b), issue(&mut c));
        assert_eq!(from_a, from_b);
        assert_ne!(from_a, from_c);
        assert_ne!(issue(&mut a), from_a);
        assert_ne!(Tokens::new().fresh("x"), Tokens::new().fresh("x"));
    }
}
