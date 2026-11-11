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
//! **Tokens survive a restart.** lsf keeps its tokens in memory, but a backend keeps the ones it
//! was given in its session storage. So each access and refresh token is sealed with a key of the
//! client id and the client secret: it holds, readable with the key only, the epoch of the
//! installation it was issued in and its rank among the tokens of its kind (offline, expiring
//! offline, client credentials, refresh) issued in that epoch, and a check of the shop, the app,
//! the kind, the epoch and the rank. Until a reset or an uninstall, an installation is in epoch
//! 0, the same every run: the n-th offline token of a run is the n-th of every run. A token lsf
//! does not know is opened ([`Tokens::recognize`]): one of an earlier run is valid again, with
//! the scopes the app is installed with now, an expiring one for an hour from then, whatever
//! order the backend presents them in.
//!
//! A reset, a shop deletion or an uninstall closes the installation: it moves to a new epoch (the
//! time of the closure, in milliseconds), and every token of an earlier epoch is refused from then
//! on, those of an earlier run lsf never saw included. The tokens issued after it are of the new
//! epoch, which no earlier run had, and they too are taken back after a restart (until the next
//! closure). A token issued in this run and since expired, migrated or retired is never taken
//! back, nor one taken back twice. Online tokens are not taken back: they last a day, and the
//! libraries exchange a session token for a new one after a `401`.
//!
//! The registry holds no lock and reads no clock: the server keeps it behind a mutex and passes
//! the time, which is the system clock (the backend compares expiries with its own).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};

use chrono::{DateTime, Duration, Utc};
use indexmap::IndexMap;
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

/// How many revoked access tokens are remembered, for their `401` to say why; the oldest are
/// forgotten first, and then answer as a token lsf never issued.
pub const REVOKED_KEPT: usize = 10_000;

/// What revoked an access token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// `DELETE /__lsf/admin`: every shop put back to the seed.
    Reset,
    /// `DELETE /__lsf/admin/shops/<shop>`.
    ShopDeleted,
    /// `POST /__lsf/admin/uninstall`, or `appUninstall`.
    Uninstalled,
}

impl Revocation {
    /// What revoked a token of `shop`, as a reason says it: `by a reset of the Admin API (…)`.
    pub fn what(self, shop: &str) -> String {
        match self {
            Revocation::Reset => "by a reset of the Admin API (DELETE /__lsf/admin)".to_string(),
            Revocation::ShopDeleted => format!(
                "when the shop was deleted (DELETE /__lsf/admin/shops/{})",
                shop.strip_suffix(".myshopify.com").unwrap_or(shop)
            ),
            Revocation::Uninstalled => {
                "when the app was uninstalled from the shop (POST /__lsf/admin/uninstall, or \
                 appUninstall)"
                    .to_string()
            }
        }
    }
}

/// A revoked access token: the shop it was for and what revoked it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revoked {
    pub shop: String,
    pub by: Revocation,
    /// Whether a refusal of it was already reported (the server says it once per token).
    told: bool,
}

impl Revoked {
    /// The `X-Lsf-Reason` of the `401` that refuses it: what revoked it, and how to get a token
    /// again. A backend that keeps its sessions sends the old one until it forgets it.
    pub fn reason(&self) -> String {
        let what = self.by.what(&self.shop);
        format!(
            "this access token of {} was revoked {what}: run the token exchange or OAuth again \
             for a new one (a backend that stored this one in its session storage must forget \
             it), or use a new shop name",
            self.shop
        )
    }
}

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
    /// Issued by an earlier run of lsf, and presented again: [`Tokens::recognize`].
    EarlierRun,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshError {
    Unknown,
    /// Issued to another app.
    OtherClient,
    /// Replaced by one the app has used since, or by a newer token acquisition.
    Retired,
    /// Past its 90 days, or 30 days after its first use.
    Expired,
    /// Its installation was closed since it was issued, in this run or an earlier one: a reset,
    /// a shop deletion or an uninstall.
    Revoked {
        shop: String,
        by: Revocation,
    },
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

/// The kinds of access tokens [`Tokens::recognize`] takes back.
const RECOGNIZED_KINDS: [&str; 3] = ["offline", "expiring", "client_credentials"];

/// How many bytes of a sealed token are its check: 48 bits, against 6 of its epoch and 4 of its
/// rank.
const CHECK_BYTES: usize = 6;
/// The epochs fit in 48 bits: milliseconds until the year 10889.
const EPOCH_MASK: u64 = (1 << 48) - 1;

/// What a token lsf does not know turns out to be ([`Tokens::recognize`]).
enum EarlierRun {
    /// Not lsf's, or this run's own (issued, then expired, migrated or retired).
    Unknown,
    /// An earlier run's, of an installation closed since.
    Closed { shop: String, by: Revocation },
    /// An earlier run's, to take back.
    Found {
        shop: String,
        client_id: String,
        kind: &'static str,
        scopes: Vec<String>,
    },
}

/// The tokens of every shop, the codes and refresh tokens, and the installations.
#[derive(Clone, Debug)]
pub struct Tokens {
    /// What codes and the `jti` of session tokens are made from: different every run.
    seed: [u8; 32],
    /// What the access and refresh tokens are derived from: the same every run of an app.
    key: [u8; 32],
    counter: u64,
    /// The rank of the next token of each kind, for each shop and app, in the epoch of its
    /// installation: `(shop, client id, prefix and kind)`.
    ranks: HashMap<(String, String, String), u32>,
    /// The epoch of each installation closed in this run, and what closed it; the others are in
    /// epoch 0 ([`Tokens::close`]).
    epochs: HashMap<(String, String), (u64, Revocation)>,
    /// The latest epoch given: each closure has a later one, even within a millisecond.
    last_epoch: u64,
    /// The tokens of an earlier run taken back in this one: never taken back again, nor issued.
    taken_back: HashSet<String>,
    access: HashMap<String, Grant>,
    refresh: HashMap<String, RefreshEntry>,
    codes: HashMap<String, Code>,
    /// The scopes the app is installed with, by shop and client id.
    installs: BTreeMap<(String, String), Vec<String>>,
    /// Migrated non-expiring tokens, for a lost answer to be sent again.
    migrations: HashMap<String, Migration>,
    /// The access tokens revoked by a reset, a shop deletion or an uninstall, the newest last:
    /// their `401` says why ([`Tokens::revocation`]).
    revoked: IndexMap<String, Revoked>,
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
        let seed = sha256(&entropy);
        Tokens::from_seed(seed, seed)
    }

    /// An empty registry whose access and refresh tokens are the same every run of the app
    /// `client_id` signed with `client_secret`: a backend's stored tokens work again after a
    /// restart ([`Tokens::recognize`]). Its codes and session token ids differ every run.
    pub fn stable(client_id: &str, client_secret: &str) -> Tokens {
        let key = hmac_sha256(
            client_secret.as_bytes(),
            format!("lsf tokens\0{client_id}").as_bytes(),
        );
        Tokens {
            key,
            ..Tokens::new()
        }
    }

    /// An empty registry whose tokens are the same every run with the same `seed`, for tests.
    pub fn with_seed(seed: u64) -> Tokens {
        let seed = sha256(&seed.to_le_bytes());
        Tokens::from_seed(seed, seed)
    }

    fn from_seed(seed: [u8; 32], key: [u8; 32]) -> Tokens {
        Tokens {
            seed,
            key,
            counter: 0,
            ranks: HashMap::new(),
            epochs: HashMap::new(),
            last_epoch: 0,
            taken_back: HashSet::new(),
            access: HashMap::new(),
            refresh: HashMap::new(),
            codes: HashMap::new(),
            installs: BTreeMap::new(),
            migrations: HashMap::new(),
            revoked: IndexMap::new(),
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

    /// The epoch tokens of `client_id` on `shop` are issued in: 0 until the installation is
    /// closed in this run.
    fn epoch(&self, shop: &str, client_id: &str) -> u64 {
        self.epochs
            .get(&(shop.to_string(), client_id.to_string()))
            .map_or(0, |(epoch, _)| *epoch)
    }

    /// The check a sealed token holds: of its prefix, kind, shop, app, epoch and rank.
    fn check(
        &self,
        prefix: &str,
        kind: &str,
        shop: &str,
        client_id: &str,
        epoch: u64,
        rank: u32,
    ) -> [u8; CHECK_BYTES] {
        let mac = hmac_sha256(
            &self.key,
            format!("{prefix}\0{kind}\0{shop}\0{client_id}\0{epoch}\0{rank}").as_bytes(),
        );
        let mut check = [0; CHECK_BYTES];
        check.copy_from_slice(&mac[..CHECK_BYTES]);
        check
    }

    /// The token of rank `rank` of `kind` for `client_id` on `shop`, in `epoch`: `prefix` and 32
    /// hex digits, which look random and which only the key opens ([`Tokens::open`]).
    fn sealed(
        &self,
        prefix: &str,
        kind: &str,
        shop: &str,
        client_id: &str,
        epoch: u64,
        rank: u32,
    ) -> String {
        let mut block = [0u8; 16];
        block[..6].copy_from_slice(&(epoch & EPOCH_MASK).to_be_bytes()[2..]);
        block[6..10].copy_from_slice(&rank.to_be_bytes());
        block[10..].copy_from_slice(&self.check(prefix, kind, shop, client_id, epoch, rank));
        format!("{prefix}{}", hex(&self.permute(block, true)))
    }

    /// The epoch, rank and check `token` holds, if it has `prefix` and 32 hex digits. Any such
    /// token opens: its check says whether lsf sealed it, and for which shop, app and kind.
    fn open(&self, token: &str, prefix: &str) -> Option<(u64, u32, [u8; CHECK_BYTES])> {
        let digits = token.strip_prefix(prefix)?;
        if digits.len() != 32 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut block = [0u8; 16];
        for (index, byte) in block.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok()?;
        }
        let block = self.permute(block, false);
        let mut epoch = [0u8; 8];
        epoch[2..].copy_from_slice(&block[..6]);
        let mut rank = [0u8; 4];
        rank.copy_from_slice(&block[6..10]);
        let mut check = [0u8; CHECK_BYTES];
        check.copy_from_slice(&block[10..]);
        Some((u64::from_be_bytes(epoch), u32::from_be_bytes(rank), check))
    }

    /// A permutation of 16 bytes keyed by the key (a Feistel network of four rounds), `forward`
    /// or back: what makes a sealed token look random.
    fn permute(&self, block: [u8; 16], forward: bool) -> [u8; 16] {
        let round = |index: u8, half: &[u8; 8]| -> [u8; 8] {
            let mut input = b"lsf token round\0".to_vec();
            input.push(index);
            input.extend_from_slice(half);
            let mac = hmac_sha256(&self.key, &input);
            let mut out = [0u8; 8];
            out.copy_from_slice(&mac[..8]);
            out
        };
        let xor = |a: [u8; 8], b: [u8; 8]| -> [u8; 8] { std::array::from_fn(|i| a[i] ^ b[i]) };
        let mut left: [u8; 8] = block[..8].try_into().unwrap_or_default();
        let mut right: [u8; 8] = block[8..].try_into().unwrap_or_default();
        if forward {
            for index in 0..4 {
                (left, right) = (right, xor(left, round(index, &right)));
            }
        } else {
            for index in (0..4).rev() {
                (left, right) = (xor(right, round(index, &left)), left);
            }
        }
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&left);
        out[8..].copy_from_slice(&right);
        out
    }

    /// The next token of `kind` for `client_id` on `shop`, in the epoch of the installation: one
    /// no earlier run's token taken back in this run already is.
    fn next(&mut self, prefix: &str, kind: &str, shop: &str, client_id: &str) -> String {
        let epoch = self.epoch(shop, client_id);
        let key = (
            shop.to_string(),
            client_id.to_string(),
            format!("{prefix}{kind}"),
        );
        let mut rank = self.ranks.get(&key).copied().unwrap_or(0);
        let mut token = self.sealed(prefix, kind, shop, client_id, epoch, rank);
        while self.taken_back.contains(&token) {
            rank += 1;
            token = self.sealed(prefix, kind, shop, client_id, epoch, rank);
        }
        self.ranks.insert(key, rank + 1);
        token
    }

    /// Closes the installation of `client_id` on `shop`, as `by` says: it moves to a new epoch,
    /// later than every one before (the time, in milliseconds, of `now`), and no token of an
    /// earlier epoch is taken back any more, an earlier run's included.
    fn close(&mut self, shop: &str, client_id: &str, by: Revocation, now: DateTime<Utc>) {
        let epoch = u64::try_from(now.timestamp_millis())
            .unwrap_or(0)
            .max(self.last_epoch + 1)
            & EPOCH_MASK;
        self.last_epoch = epoch;
        self.epochs
            .insert((shop.to_string(), client_id.to_string()), (epoch, by));
        self.ranks
            .retain(|(ranked, client, _), _| !(ranked == shop && client == client_id));
    }

    /// Whether `token` (of `prefix`) is one lsf would issue for an app on a shop, and may take
    /// back: an earlier run's, of one of `kinds`, which this run has neither issued nor taken back,
    /// for an installation in place.
    fn earlier_run(&self, token: &str, prefix: &str, kinds: &[&'static str]) -> EarlierRun {
        let Some((epoch, rank, check)) = self.open(token, prefix) else {
            return EarlierRun::Unknown;
        };
        let mut candidates: Vec<(String, String)> = self.installs.keys().cloned().collect();
        candidates.extend(
            self.epochs
                .keys()
                .filter(|key| !self.installs.contains_key(*key))
                .cloned(),
        );
        for (shop, client_id) in candidates {
            for kind in kinds {
                if self.check(prefix, kind, &shop, &client_id, epoch, rank) != check {
                    continue;
                }
                let current = self.epochs.get(&(shop.clone(), client_id.clone()));
                if let Some((closed_at, by)) = current {
                    if epoch < *closed_at {
                        return EarlierRun::Closed { shop, by: *by };
                    }
                    // The epoch of this run's closure: only this run issued tokens in it.
                    if epoch == *closed_at {
                        return EarlierRun::Unknown;
                    }
                } else if epoch == 0 {
                    // Epoch 0 is every run's: the ranks this run issued are its own tokens.
                    let issued = self
                        .ranks
                        .get(&(shop.clone(), client_id.clone(), format!("{prefix}{kind}")))
                        .copied()
                        .unwrap_or(0);
                    if rank < issued {
                        return EarlierRun::Unknown;
                    }
                }
                let Some(scopes) = self.installs.get(&(shop.clone(), client_id.clone())) else {
                    return EarlierRun::Unknown;
                };
                if self.taken_back.contains(token) {
                    return EarlierRun::Unknown;
                }
                return EarlierRun::Found {
                    shop,
                    client_id,
                    kind,
                    scopes: scopes.clone(),
                };
            }
        }
        EarlierRun::Unknown
    }

    /// Takes back an access token an earlier run of lsf issued, which a backend kept: offline,
    /// expiring or of client credentials, for an app installed on a shop whose installation was
    /// not closed since (see the module). It grants what the installation grants now; an
    /// expiring one lives an hour from `now`, a client credentials one a day. Whether it was. A
    /// token of an installation closed since is remembered as revoked by what closed it
    /// ([`Tokens::revocation`]).
    pub fn recognize(&mut self, token: &str, now: DateTime<Utc>) -> bool {
        let token = token.trim();
        if !token.starts_with("shpat_")
            || self.access.contains_key(token)
            || self.revoked.contains_key(token)
        {
            return false;
        }
        match self.earlier_run(token, "shpat_", &RECOGNIZED_KINDS) {
            EarlierRun::Unknown => false,
            EarlierRun::Closed { shop, by } => {
                self.remember_revoked(vec![token.to_string()], &shop, by);
                false
            }
            EarlierRun::Found {
                shop,
                client_id,
                kind,
                scopes,
            } => {
                let expires_at = match kind {
                    "expiring" => Some(now + Duration::seconds(EXPIRING_OFFLINE_LIFETIME)),
                    "client_credentials" => {
                        Some(now + Duration::seconds(CLIENT_CREDENTIALS_LIFETIME))
                    }
                    _ => None,
                };
                self.taken_back.insert(token.to_string());
                self.access.insert(
                    token.to_string(),
                    Grant {
                        shop,
                        client_id,
                        scopes,
                        mode: AccessMode::Offline,
                        user: None,
                        issued_at: now,
                        expires_at,
                        source: GrantSource::EarlierRun,
                    },
                );
                true
            }
        }
    }

    /// Takes back a refresh token an earlier run issued, as [`Tokens::recognize`] does an
    /// access token: usable for 90 days from `now`. What closed its installation since, if
    /// something did.
    fn recognize_refresh(
        &mut self,
        refresh_token: &str,
        now: DateTime<Utc>,
    ) -> Result<(), RefreshError> {
        if !refresh_token.starts_with("shprt_") || self.refresh.contains_key(refresh_token) {
            return Ok(());
        }
        match self.earlier_run(refresh_token, "shprt_", &["refresh"]) {
            EarlierRun::Unknown => Ok(()),
            EarlierRun::Closed { shop, by } => Err(RefreshError::Revoked { shop, by }),
            EarlierRun::Found {
                shop,
                client_id,
                scopes,
                ..
            } => {
                self.taken_back.insert(refresh_token.to_string());
                self.refresh.insert(
                    refresh_token.to_string(),
                    RefreshEntry {
                        shop,
                        client_id,
                        scopes,
                        expires_at: now + Duration::seconds(REFRESH_TOKEN_LIFETIME),
                        first_used_at: None,
                        predecessor: None,
                        retired: None,
                    },
                );
                Ok(())
            }
        }
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
        // Installed again: the default token is valid again.
        self.revoked.shift_remove(&token);
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

    /// Uninstalls `client_id` from `shop`: its tokens, refresh tokens and codes stop working,
    /// and no token of an earlier run is taken back for it any more.
    /// What `app/uninstalled` reports. Answers the number of access tokens revoked.
    pub fn uninstall(&mut self, shop: &str, client_id: &str, now: DateTime<Utc>) -> usize {
        self.installs
            .remove(&(shop.to_string(), client_id.to_string()));
        self.close(shop, client_id, Revocation::Uninstalled, now);
        let before = self.access.len();
        let gone: Vec<String> = self
            .access
            .iter()
            .filter(|(_, grant)| grant.shop == shop && grant.client_id == client_id)
            .map(|(token, _)| token.clone())
            .collect();
        self.remember_revoked(gone, shop, Revocation::Uninstalled);
        self.access
            .retain(|_, grant| !(grant.shop == shop && grant.client_id == client_id));
        self.refresh
            .retain(|_, entry| !(entry.shop == shop && entry.client_id == client_id));
        self.codes
            .retain(|_, code| !(code.shop == shop && code.client_id == client_id));
        before - self.access.len()
    }

    /// Revokes every token of every app on `shop`, and its installations: a shop reset or
    /// deleted, as `by` says: no token of an earlier run is taken back for them any more. Answers
    /// the number of access tokens revoked.
    pub fn revoke_shop(&mut self, shop: &str, by: Revocation, now: DateTime<Utc>) -> usize {
        let mut clients: Vec<String> = self
            .installs
            .keys()
            .chain(self.epochs.keys())
            .filter(|(installed, _)| installed == shop)
            .map(|(_, client_id)| client_id.clone())
            .collect();
        clients.extend(
            self.access
                .values()
                .filter(|grant| grant.shop == shop)
                .map(|grant| grant.client_id.clone()),
        );
        clients.sort();
        clients.dedup();
        for client_id in clients {
            self.close(shop, &client_id, by, now);
        }
        self.installs.retain(|(installed, _), _| installed != shop);
        let before = self.access.len();
        let gone: Vec<String> = self
            .access
            .iter()
            .filter(|(_, grant)| grant.shop == shop)
            .map(|(token, _)| token.clone())
            .collect();
        self.remember_revoked(gone, shop, by);
        self.access.retain(|_, grant| grant.shop != shop);
        self.refresh.retain(|_, entry| entry.shop != shop);
        self.codes.retain(|_, code| code.shop != shop);
        before - self.access.len()
    }

    /// Keeps what revoked `tokens`, the newest last, at most [`REVOKED_KEPT`].
    fn remember_revoked(&mut self, tokens: Vec<String>, shop: &str, by: Revocation) {
        for token in tokens {
            self.revoked.shift_remove(&token);
            self.revoked.insert(
                token,
                Revoked {
                    shop: shop.to_string(),
                    by,
                    told: false,
                },
            );
        }
        while self.revoked.len() > REVOKED_KEPT {
            self.revoked.shift_remove_index(0);
        }
    }

    /// What revoked `token`, when a reset, a shop deletion or an uninstall did and it was not
    /// issued again since: why [`Tokens::lookup`] finds nothing.
    pub fn revocation(&self, token: &str) -> Option<&Revoked> {
        if self.access.contains_key(token.trim()) {
            return None;
        }
        self.revoked.get(token.trim())
    }

    /// Notes that a refusal of the revoked `token` was reported: `true` the first time only.
    pub fn first_refusal(&mut self, token: &str) -> bool {
        match self.revoked.get_mut(token.trim()) {
            Some(revoked) if !revoked.told => {
                revoked.told = true;
                true
            }
            _ => false,
        }
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
        let refresh_token = self.next("shprt_", "refresh", shop, client_id);
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
        let kind = match (grant.mode, grant.source, grant.expires_at) {
            (AccessMode::Online, _, _) => {
                format!("online:{}", grant.user.as_ref().map_or(0, |user| user.id))
            }
            (_, GrantSource::ClientCredentials, _) => "client_credentials".to_string(),
            (_, _, Some(_)) => "expiring".to_string(),
            (_, _, None) => "offline".to_string(),
        };
        let access_token = self.next("shpat_", &kind, &grant.shop, &grant.client_id);
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
        self.recognize_refresh(refresh_token, now)?;
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
        self.recognize(token, now);
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
        assert_eq!(tokens.uninstall(SHOP, "client", at(100)), 2);
        assert_eq!(tokens.lookup(&default, at(1)), Err(TokenError::Unknown));
        assert_eq!(
            tokens.refresh(issued.refresh_token.as_deref().unwrap(), "client", at(1)),
            Err(RefreshError::Revoked {
                shop: SHOP.to_string(),
                by: Revocation::Uninstalled
            })
        );
        assert_eq!(tokens.installed_scopes(SHOP, "client"), None);
        assert!(tokens.lookup(&other_shop, at(1)).is_ok());
        assert_eq!(
            tokens.revoke_shop("other.myshopify.com", Revocation::ShopDeleted, at(100)),
            1
        );
        assert!(tokens.grants().next().is_none());
    }

    #[test]
    fn a_revoked_token_remembers_what_revoked_it_until_it_is_issued_again() {
        let mut tokens = Tokens::with_seed(1);
        let default = tokens.install(SHOP, "client", &scopes(), at(0));
        let exchanged = tokens
            .issue_offline(
                SHOP,
                "client",
                &scopes(),
                false,
                GrantSource::TokenExchange,
                at(0),
            )
            .access_token;
        assert_eq!(tokens.revocation(&exchanged), None);
        tokens.revoke_shop(SHOP, Revocation::Reset, at(100));
        // A reset installs the app again: its default token is valid again, not the other.
        tokens.install(SHOP, "client", &scopes(), at(1));
        assert!(tokens.lookup(&default, at(1)).is_ok());
        assert_eq!(tokens.revocation(&default), None);
        let revoked = tokens.revocation(&exchanged).unwrap().clone();
        assert_eq!(revoked.by, Revocation::Reset);
        assert_eq!(
            revoked.reason(),
            "this access token of my-shop.myshopify.com was revoked by a reset of the Admin API \
             (DELETE /__lsf/admin): run the token exchange or OAuth again for a new one (a \
             backend that stored this one in its session storage must forget it), or use a new \
             shop name"
        );
        // Said once.
        assert!(tokens.first_refusal(&exchanged));
        assert!(!tokens.first_refusal(&exchanged));
        assert!(!tokens.first_refusal("shpat_never_issued"));

        // An uninstall, then a deleted shop: the latest says why.
        tokens.uninstall(SHOP, "client", at(100));
        assert_eq!(
            tokens.revocation(&default).map(|revoked| revoked.by),
            Some(Revocation::Uninstalled)
        );
        let test = tokens.install("test-1.myshopify.com", "client", &scopes(), at(2));
        tokens.revoke_shop("test-1.myshopify.com", Revocation::ShopDeleted, at(100));
        assert!(
            tokens
                .revocation(&test)
                .unwrap()
                .reason()
                .contains("when the shop was deleted (DELETE /__lsf/admin/shops/test-1)")
        );
        assert_eq!(tokens.revocation("shpat_never_issued"), None);
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
        assert_ne!(Tokens::new().entropy("x"), Tokens::new().entropy("x"));
    }

    /// A run of lsf with the app installed on [`SHOP`], as the server starts.
    fn run(secret: &str) -> Tokens {
        let mut tokens = Tokens::stable("client", secret);
        tokens.install(SHOP, "client", &scopes(), at(0));
        tokens
    }

    #[test]
    fn a_token_of_an_earlier_run_is_valid_again() {
        let mut first = run("secret");
        let offline = first
            .issue_offline(
                SHOP,
                "client",
                &scopes(),
                false,
                GrantSource::TokenExchange,
                at(0),
            )
            .access_token;
        let expiring = first.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(0),
        );
        // lsf restarts: the backend still has both.
        let mut second = run("secret");
        assert_eq!(second.lookup(&offline, at(10)), Err(TokenError::Unknown));
        assert!(second.recognize(&offline, at(10)));
        let grant = second.lookup(&offline, at(10)).unwrap();
        assert_eq!(grant.source, GrantSource::EarlierRun);
        assert_eq!(grant.expires_at, None);
        assert!(second.recognize(&expiring.access_token, at(10)));
        assert_eq!(
            second
                .lookup(&expiring.access_token, at(10))
                .unwrap()
                .expires_at,
            Some(at(10 + EXPIRING_OFFLINE_LIFETIME))
        );
        // Its refresh token too, and what it gives is new.
        let refreshed = second
            .refresh(expiring.refresh_token.as_deref().unwrap(), "client", at(20))
            .unwrap();
        assert_ne!(
            Some(refreshed.access_token.clone()),
            Some(expiring.access_token.clone())
        );
        // The next token of the run is not one taken back.
        let next = second
            .issue_offline(
                SHOP,
                "client",
                &scopes(),
                false,
                GrantSource::TokenExchange,
                at(30),
            )
            .access_token;
        assert_ne!(next, offline);
    }

    #[test]
    fn a_token_revoked_or_of_another_secret_is_not_taken_back() {
        let mut first = run("secret");
        let offline = first
            .issue_offline(
                SHOP,
                "client",
                &scopes(),
                false,
                GrantSource::TokenExchange,
                at(0),
            )
            .access_token;
        assert!(!run("another secret").recognize(&offline, at(1)));
        assert!(
            !run("secret").recognize(&format!("shpat_{}", "0123456789abcdef".repeat(2)), at(1))
        );
        // Uninstalled in this run: revoked, even once installed again.
        first.uninstall(SHOP, "client", at(100));
        first.install(SHOP, "client", &scopes(), at(2));
        assert!(!first.recognize(&offline, at(3)));
        assert_eq!(first.lookup(&offline, at(3)), Err(TokenError::Unknown));
        // Not installed: nothing to take back.
        let mut bare = Tokens::stable("client", "secret");
        assert!(!bare.recognize(&offline, at(1)));
    }

    #[test]
    fn a_token_expired_in_this_run_is_not_taken_back() {
        let mut tokens = run("secret");
        let issued = tokens.issue_offline(
            SHOP,
            "client",
            &scopes(),
            true,
            GrantSource::TokenExchange,
            at(0),
        );
        tokens.purge(at(2 * EXPIRING_OFFLINE_LIFETIME));
        assert!(!tokens.recognize(&issued.access_token, at(2 * EXPIRING_OFFLINE_LIFETIME)));
    }

    /// What a backend kept from a run of lsf: two offline tokens, two expiring ones and their
    /// refresh tokens, in the order they were issued.
    fn kept(tokens: &mut Tokens) -> (Vec<String>, Vec<String>, Vec<String>) {
        let offline = (0..2)
            .map(|_| {
                tokens
                    .issue_offline(
                        SHOP,
                        "client",
                        &scopes(),
                        false,
                        GrantSource::TokenExchange,
                        at(0),
                    )
                    .access_token
            })
            .collect();
        let (mut expiring, mut refresh) = (Vec::new(), Vec::new());
        for _ in 0..2 {
            let issued = tokens.issue_offline(
                SHOP,
                "client",
                &scopes(),
                true,
                GrantSource::Migration,
                at(0),
            );
            expiring.push(issued.access_token);
            refresh.push(issued.refresh_token.unwrap());
        }
        (offline, expiring, refresh)
    }

    #[test]
    fn tokens_of_an_earlier_run_are_taken_back_in_any_order() {
        let (offline, expiring, refresh) = kept(&mut run("secret"));
        let mut second = run("secret");
        // The second first: the first is still taken back.
        for token in [&offline[1], &offline[0], &expiring[1], &expiring[0]] {
            assert!(second.recognize(token, at(10)), "{token}");
            assert!(second.lookup(token, at(10)).is_ok());
        }
        for token in [&refresh[1], &refresh[0]] {
            assert!(second.refresh(token, "client", at(20)).is_ok(), "{token}");
        }
        // Taken back once: a new token of the run is none of them.
        let next = second
            .issue_offline(
                SHOP,
                "client",
                &scopes(),
                false,
                GrantSource::TokenExchange,
                at(30),
            )
            .access_token;
        assert!(!offline.contains(&next));
    }

    #[test]
    fn a_reset_or_an_uninstall_stops_taking_back_tokens_of_an_earlier_run() {
        for by in [
            Revocation::Reset,
            Revocation::Uninstalled,
            Revocation::ShopDeleted,
        ] {
            let (offline, expiring, refresh) = kept(&mut run("secret"));
            // lsf restarts; the backend presents one token, then the rest after the reset.
            let mut second = run("secret");
            assert!(second.recognize(&offline[0], at(10)));
            match by {
                Revocation::Uninstalled => {
                    second.uninstall(SHOP, "client", at(20));
                }
                _ => {
                    second.revoke_shop(SHOP, by, at(20));
                }
            }
            second.install(SHOP, "client", &scopes(), at(21));
            for token in [&offline[0], &offline[1], &expiring[0], &expiring[1]] {
                assert!(!second.recognize(token, at(30)), "{by:?} {token}");
                assert_eq!(second.lookup(token, at(30)), Err(TokenError::Unknown));
                assert_eq!(second.revocation(token).map(|revoked| revoked.by), Some(by));
            }
            for token in &refresh {
                assert_eq!(
                    second.refresh(token, "client", at(30)),
                    Err(RefreshError::Revoked {
                        shop: SHOP.to_string(),
                        by
                    })
                );
            }
            // A token issued after it works, is none of the earlier run's, and is taken back
            // after the next restart.
            let issued = second.issue_offline(
                SHOP,
                "client",
                &scopes(),
                true,
                GrantSource::TokenExchange,
                at(40),
            );
            assert!(second.lookup(&issued.access_token, at(41)).is_ok());
            assert!(!expiring.contains(&issued.access_token));
            let fresh = issued.refresh_token.unwrap();
            assert!(!refresh.contains(&fresh));
            let mut third = run("secret");
            assert!(third.recognize(&issued.access_token, at(50)));
            assert!(third.refresh(&fresh, "client", at(50)).is_ok());
            // A run that has not closed the installation takes back any run's tokens.
            assert!(third.recognize(&offline[1], at(50)));
        }
    }

    #[test]
    fn a_token_migrated_in_this_run_is_not_taken_back() {
        let mut first = run("secret");
        let offline = first
            .issue_offline(
                SHOP,
                "client",
                &scopes(),
                false,
                GrantSource::TokenExchange,
                at(0),
            )
            .access_token;
        let mut second = run("secret");
        assert!(second.recognize(&offline, at(10)));
        second.migrate(&offline, "client", SHOP, at(20)).unwrap();
        assert!(!second.recognize(&offline, at(30)));
        assert_eq!(second.lookup(&offline, at(30)), Err(TokenError::Unknown));
    }

    #[test]
    fn a_token_not_sealed_with_the_key_opens_as_nothing() {
        let tokens = run("secret");
        assert!(tokens.open("shpat_0123", "shpat_").is_none());
        assert!(
            tokens
                .open("shpat_zz23456789abcdef0123456789abcdef", "shpat_")
                .is_none()
        );
        let sealed = tokens.sealed("shpat_", "offline", SHOP, "client", 1_234, 5);
        let (epoch, rank, check) = tokens.open(&sealed, "shpat_").unwrap();
        assert_eq!((epoch, rank), (1_234, 5));
        assert_eq!(
            check,
            tokens.check("shpat_", "offline", SHOP, "client", 1_234, 5)
        );
        // One digit changed: the check no longer matches.
        let mut forged = sealed.clone().into_bytes();
        forged[10] = if forged[10] == b'0' { b'1' } else { b'0' };
        let forged = String::from_utf8(forged).unwrap();
        assert!(!run("secret").recognize(&forged, at(1)));
    }
}
