//! Store credit: the accounts of customers and company locations, what credits and debits
//! them, and store credit as a way to pay and to refund an order.
//!
//! As shopify.dev's reference of 2026-10 describes it (`mutations/storeCreditAccountCredit`,
//! `mutations/storeCreditAccountDebit`, `queries/storeCreditAccount`, `objects/
//! StoreCreditAccount…Transaction`):
//!
//! - An account holds one currency; an owner (a `Customer` or a `CompanyLocation`) has one per
//!   currency. Crediting an owner makes its account of the currency when it has none; debiting
//!   one needs the account.
//! - **Credits** may expire (`expiresAt`); each keeps what is left of it (`remainingAmount`).
//!   **Debits** take from the credits that expire first, then from the oldest. A credit that
//!   expires with something left makes a `StoreCreditAccountExpirationTransaction` of it when
//!   the shop's clock passes its date ([`tick`]). `balanceAfterTransaction` is the balance each
//!   left.
//! - Limits: the amount must be positive ("A positive amount must be used to credit a store
//!   credit account", the reference's example), a debit at most the balance ("The store credit
//!   account does not have sufficient funds to satisfy the request"), and a balance below 100,000
//!   in the account's currency ("The operation would cause the account's credit limit to be
//!   exceeded"; Shopify's limit depends on the currency and is not published: the example
//!   refuses a credit of 100,000.00 USD, lsf refuses any balance from there).
//! - **Paying with store credit**: a transaction of `orderCreate` through the gateway
//!   `shopify_store_credit` debits the customer's account (event `ORDER_PAYMENT`, its origin the
//!   order's transaction), as the checkout does when a buyer pays with store credit.
//! - **Refunding to store credit**: `refundCreate`'s and `returnProcess`'s
//!   `refundMethods.storeCreditRefund`, and `orderCancel`'s `refundMethod.storeCreditRefund`, credit
//!   the customer's account (events `ORDER_REFUND`, `ORDER_CANCELLATION`) through a `REFUND`
//!   transaction of the gateway `shopify_store_credit`.
//! - `notify: true` puts the notice in the shop's outbox ([`crate::outbox`]).
//!
//! The 2026-10 webhook topics have none for store credit: the changes emit
//! `domains::app::CHANGED`, which no subscription receives.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::model::{self, text};
use crate::domains::app::CHANGED;
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::outbox::{self, Entry};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("storeCreditAccountCredit", credit_mutation)
        .mutation("storeCreditAccountDebit", debit_mutation)
        .search_field(
            "StoreCreditAccount",
            "currency_code",
            |_r, _item, account| {
                Some(
                    account
                        .get("balance")
                        .and_then(|balance| balance.get("currencyCode"))
                        .and_then(Value::as_str)
                        .map(|code| SearchValue::Keyword(code.to_string()))
                        .into_iter()
                        .collect(),
                )
            },
        )
        // The connection's node is the interface: the type of each transaction is its own.
        .search_field(
            "StoreCreditAccountTransaction",
            "type",
            |_r, _item, transaction| {
                let kind = match text(transaction, "__typename") {
                    Some("StoreCreditAccountCreditTransaction") => "credit",
                    Some("StoreCreditAccountDebitTransaction") => "debit",
                    Some("StoreCreditAccountDebitRevertTransaction") => "debit_revert",
                    Some("StoreCreditAccountExpirationTransaction") => "expiration",
                    _ => return Some(Vec::new()),
                };
                Some(vec![SearchValue::Keyword(kind.into())])
            },
        );
}

/// The gateway of a payment or a refund in store credit.
pub const GATEWAY: &str = "shopify_store_credit";

/// The balance an account may not reach, in cents of its currency.
const CREDIT_LIMIT: i64 = 10_000_000;

const NOT_ENOUGH: &str =
    "The store credit account does not have sufficient funds to satisfy the request";

/// Why an account cannot be found or made.
pub enum AccountError {
    /// The id names an account the shop does not have.
    AccountNotFound,
    /// The id names an owner the shop does not have.
    OwnerNotFound,
    /// The owner has no account of the currency (a debit makes none).
    NoAccount,
    /// The currency is not one the shop sells in.
    UnsupportedCurrency,
}

/// The account an id names (an account, or an owner and the currency), made for an owner that
/// has none when `create`.
pub fn account_of(
    state: &mut ShopState,
    id: &str,
    currency: &str,
    create: bool,
) -> Result<String, AccountError> {
    match state.type_of(id) {
        Some("StoreCreditAccount") => return Ok(id.to_string()),
        Some("Customer" | "CompanyLocation") => {}
        _ if crate::state::gid_type(id) == Some("StoreCreditAccount") => {
            return Err(AccountError::AccountNotFound);
        }
        _ => return Err(AccountError::OwnerNotFound),
    }
    let existing = state.get(id).and_then(|owner| {
        gids(owner.get("storeCreditAccounts"))
            .into_iter()
            .find(|account| {
                state
                    .get(account)
                    .is_some_and(|record| currency_of(record) == Some(currency))
            })
    });
    if let Some(account) = existing {
        return Ok(account);
    }
    if !create {
        return Err(AccountError::NoAccount);
    }
    if !supported(state, currency) {
        return Err(AccountError::UnsupportedCurrency);
    }
    let account = state.new_gid("StoreCreditAccount");
    state.insert(value::object(json!({
        "__typename": "StoreCreditAccount",
        "id": account,
        "owner": id,
        "balance": value::money_v2(0, currency),
        "transactions": [],
    })));
    model::push_key(state, id, "storeCreditAccounts", &account);
    Ok(account)
}

/// The currency of an account.
fn currency_of(account: &Record) -> Option<&str> {
    account.get("balance")?.get("currencyCode")?.as_str()
}

/// Whether the shop sells in a currency: its own, or one of its presentment currencies.
fn supported(state: &ShopState, currency: &str) -> bool {
    model::currency(state) == currency
        || state
            .shop()
            .and_then(|shop| shop.get("enabledPresentmentCurrencies"))
            .and_then(Value::as_array)
            .is_some_and(|codes| codes.iter().any(|code| code.as_str() == Some(currency)))
}

/// The balance of an account, in cents.
pub fn balance(state: &ShopState, account: &str) -> i64 {
    state
        .get(account)
        .map_or(0, |record| model::cents(record.get("balance")))
}

/// Writes an account's balance and appends a transaction to it.
fn add_to_account(state: &mut ShopState, account: &str, transaction: Value, balance: i64) {
    let currency = state
        .get(account)
        .and_then(|record| currency_of(record))
        .unwrap_or("USD")
        .to_string();
    if let Some(record) = state.get_mut(account) {
        record.insert("balance".into(), value::money_v2(balance, &currency));
        if let Some(Value::Array(items)) = record.get_mut("transactions") {
            items.push(transaction);
        }
    }
}

/// Credits an account: a `StoreCreditAccountCreditTransaction` of `amount` (cents), expiring at
/// `expires_at`. Returns its GID.
pub fn credit(
    state: &mut ShopState,
    account: &str,
    amount: i64,
    expires_at: Option<&str>,
    event: &str,
    origin: Option<&str>,
) -> String {
    let currency = state
        .get(account)
        .and_then(|record| currency_of(record))
        .unwrap_or("USD")
        .to_string();
    let after = balance(state, account) + amount;
    let id = state.new_gid("StoreCreditAccountCreditTransaction");
    state.insert(value::object(json!({
        "__typename": "StoreCreditAccountCreditTransaction",
        "id": id,
        "account": account,
        "amount": value::money_v2(amount, &currency),
        "balanceAfterTransaction": value::money_v2(after, &currency),
        "remainingAmount": value::money_v2(amount, &currency),
        "expiresAt": expires_at,
        "event": event,
        "origin": origin,
        "createdAt": state.timestamp(),
    })));
    add_to_account(state, account, json!(id), after);
    state.emit(CHANGED, &id);
    id
}

/// The credits of an account with something left, in the order a debit takes from them: those
/// that expire first, then the oldest.
fn credits_left(state: &ShopState, account: &str) -> Vec<(String, i64)> {
    let mut credits: Vec<(Option<String>, usize, String, i64)> = state
        .get(account)
        .map(|record| gids(record.get("transactions")))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(position, id)| {
            let record = state.get(&id)?;
            (text(record, "__typename") == Some("StoreCreditAccountCreditTransaction")).then(|| {
                (
                    text(record, "expiresAt").map(str::to_string),
                    position,
                    id.clone(),
                    model::cents(record.get("remainingAmount")),
                )
            })
        })
        .filter(|(_, _, _, left)| *left > 0)
        .collect();
    // Expiring credits first (soonest first), then those that never expire, oldest first.
    credits.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y).then(a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    credits
        .into_iter()
        .map(|(_, _, id, left)| (id, left))
        .collect()
}

/// Debits an account: a `StoreCreditAccountDebitTransaction` of `amount` (cents), taken from its
/// credits. `None` when the balance cannot cover it.
pub fn debit(
    state: &mut ShopState,
    account: &str,
    amount: i64,
    event: &str,
    origin: Option<&str>,
) -> Option<String> {
    let before = balance(state, account);
    if amount > before {
        return None;
    }
    let currency = state
        .get(account)
        .and_then(|record| currency_of(record))
        .unwrap_or("USD")
        .to_string();
    let mut left = amount;
    for (credit, remaining) in credits_left(state, account) {
        if left <= 0 {
            break;
        }
        let taken = remaining.min(left);
        left -= taken;
        if let Some(record) = state.get_mut(&credit) {
            record.insert(
                "remainingAmount".into(),
                value::money_v2(remaining - taken, &currency),
            );
        }
    }
    let id = state.new_gid("StoreCreditAccountDebitTransaction");
    state.insert(value::object(json!({
        "__typename": "StoreCreditAccountDebitTransaction",
        "id": id,
        "account": account,
        "amount": value::money_v2(-amount, &currency),
        "balanceAfterTransaction": value::money_v2(before - amount, &currency),
        "event": event,
        "origin": origin,
        "createdAt": state.timestamp(),
    })));
    add_to_account(state, account, json!(id), before - amount);
    state.emit(CHANGED, &id);
    Some(id)
}

/// Expires what is left of the credits whose date the shop's clock passed: a
/// `StoreCreditAccountExpirationTransaction` of it (inline: the type is not a `Node`), the
/// account's balance down by as much.
pub fn tick(state: &mut ShopState) {
    let now = state.now();
    let expired: Vec<(String, String, i64)> = state
        .keys("StoreCreditAccountCreditTransaction")
        .filter_map(|key| {
            let record = state.get(key)?;
            let expires = DateTime::parse_from_rfc3339(text(record, "expiresAt")?).ok()?;
            let left = model::cents(record.get("remainingAmount"));
            (left > 0 && expires.with_timezone(&Utc) <= now).then(|| {
                (
                    key.to_string(),
                    text(record, "account").unwrap_or_default().to_string(),
                    left,
                )
            })
        })
        .collect();
    for (credit, account, left) in expired {
        let currency = state
            .get(&account)
            .and_then(|record| currency_of(record))
            .unwrap_or("USD")
            .to_string();
        let after = balance(state, &account) - left;
        let at = state
            .get(&credit)
            .and_then(|record| text(record, "expiresAt"))
            .map(str::to_string);
        if let Some(record) = state.get_mut(&credit) {
            record.insert("remainingAmount".into(), value::money_v2(0, &currency));
        }
        let expiration = json!({
            "__typename": "StoreCreditAccountExpirationTransaction",
            "account": account,
            "amount": value::money_v2(-left, &currency),
            "balanceAfterTransaction": value::money_v2(after, &currency),
            "creditTransaction": credit,
            "event": "ADJUSTMENT",
            "origin": null,
            "createdAt": at,
        });
        add_to_account(state, &account, expiration, after);
        state.emit(CHANGED, &account);
    }
}

/// The account of a payment or a refund of an order in store credit: its customer's, in the
/// shop's currency (made for a refund).
fn order_account(state: &mut ShopState, order: &str, create: bool) -> Option<String> {
    let customer = state
        .get(order)
        .and_then(|record| text(record, "customer"))
        .map(str::to_string)?;
    let currency = model::currency(state);
    account_of(state, &customer, &currency, create).ok()
}

/// Whether an order's customer has the store credit to pay `amount` (cents) with.
pub fn can_pay(state: &ShopState, customer: Option<&str>, amount: i64) -> bool {
    let Some(customer) = customer else {
        return false;
    };
    let currency = model::currency(state);
    // Reading does not make an account: a savepoint of the caller's mutation would undo it.
    let account = state.get(customer).and_then(|owner| {
        gids(owner.get("storeCreditAccounts"))
            .into_iter()
            .find(|account| {
                state
                    .get(account)
                    .is_some_and(|record| currency_of(record) == Some(currency.as_str()))
            })
    });
    account.is_some_and(|account| balance(state, &account) >= amount)
}

/// Debits the customer's account for a payment of an order in store credit (`ORDER_PAYMENT`,
/// its origin the order's transaction). Returns whether it could.
pub fn pay(state: &mut ShopState, order: &str, transaction: &str, amount: i64) -> bool {
    let Some(account) = order_account(state, order, false) else {
        return false;
    };
    debit(state, &account, amount, "ORDER_PAYMENT", Some(transaction)).is_some()
}

/// Credits the customer's account with a refund of an order (`ORDER_REFUND`, or
/// `ORDER_CANCELLATION` for a cancellation), its origin the refund's transaction.
pub fn refund(
    state: &mut ShopState,
    order: &str,
    transaction: &str,
    amount: i64,
    expires_at: Option<&str>,
    event: &str,
) {
    if let Some(account) = order_account(state, order, true) {
        credit(
            state,
            &account,
            amount,
            expires_at,
            event,
            Some(transaction),
        );
    }
}

/// A `StoreCreditRefundInput` (`{amount: MoneyInput, expiresAt}`), checked against the order:
/// it must have a customer, the amount be positive, the date in the future. `(cents,
/// expiresAt)`.
pub fn read_refund(
    state: &ShopState,
    order: &Record,
    input: &Map<String, Value>,
    path: &[&str],
) -> Result<(i64, Option<String>), UserError> {
    if text(order, "customer").is_none() {
        // lsf's wording, as the others of store credit refunds: shopify.dev shows none.
        return Err(UserError::new(
            path,
            "A refund to store credit needs the order to have a customer",
        ));
    }
    let amount = model::cents(input.get("amount"));
    if amount <= 0 {
        return Err(UserError::new(
            &[path, &["amount"]].concat(),
            "A positive amount must be used to credit a store credit account",
        ));
    }
    if let Some(currency) = input
        .get("amount")
        .and_then(|amount| amount.get("currencyCode"))
        .and_then(Value::as_str)
        && currency != model::currency(state)
    {
        return Err(UserError::new(
            &[path, &["amount", "currencyCode"]].concat(),
            "The currency must be the order's currency",
        ));
    }
    let expires_at = match input.get("expiresAt").filter(|value| !value.is_null()) {
        None => None,
        Some(at) => Some(future_date(state, at).ok_or_else(|| {
            UserError::new(
                &[path, &["expiresAt"]].concat(),
                "The expiry date must be in the future",
            )
        })?),
    };
    Ok((amount, expires_at))
}

/// A `DateTime` (or a `Date`, at midnight UTC) after the shop's time now, as a `DateTime`.
fn future_date(state: &ShopState, at: &Value) -> Option<String> {
    let text = at.as_str()?;
    let at = DateTime::parse_from_rfc3339(text)
        .map(|at| at.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|day| day.and_hms_opt(0, 0, 0))
                .map(|at| at.and_utc())
        })?;
    (at > state.now()).then(|| value::date_time(at))
}

/// The cents and the currency of a `MoneyInput`.
fn money(input: Option<&Value>) -> (i64, Option<String>) {
    (
        model::cents(input),
        input
            .and_then(|money| money.get("currencyCode"))
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// The owner's email, for a notice of the outbox.
fn owner_email(state: &ShopState, account: &str) -> Option<String> {
    let owner = state
        .get(account)
        .and_then(|record| text(record, "owner"))?;
    let record = state.get(owner)?;
    text(record, "email").map(str::to_string)
}

/// `storeCreditAccountCredit`.
fn credit_mutation(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"storeCreditAccountTransaction": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = model::input_of(ctx.arg("creditInput"));
    let (amount, currency) = money(input.get("creditAmount"));
    let currency = currency.unwrap_or_else(|| model::currency(mutation.state()));
    let amount_path = ["creditInput", "creditAmount", "amount"];
    if amount <= 0 {
        return failed(
            &amount_path,
            "A positive amount must be used to credit a store credit account",
            "NEGATIVE_OR_ZERO_AMOUNT",
        );
    }
    let expires_at = match input.get("expiresAt").filter(|value| !value.is_null()) {
        None => None,
        Some(at) => match future_date(mutation.state(), at) {
            Some(at) => Some(at),
            None => {
                return failed(
                    &["creditInput", "expiresAt"],
                    "The expiry date must be in the future",
                    "EXPIRES_AT_IN_PAST",
                );
            }
        },
    };
    // The account is made in the mutation's savepoint: a refusal below takes it back.
    let account = match account_of(mutation.state_mut(), &id, &currency, true) {
        Ok(account) => account,
        Err(AccountError::AccountNotFound | AccountError::NoAccount) => {
            return failed(
                &["id"],
                "Store credit account does not exist",
                "ACCOUNT_NOT_FOUND",
            );
        }
        Err(AccountError::OwnerNotFound) => {
            return failed(&["id"], "Owner does not exist", "OWNER_NOT_FOUND");
        }
        Err(AccountError::UnsupportedCurrency) => {
            return failed(
                &["creditInput", "creditAmount", "currencyCode"],
                "The currency is not supported by the shop",
                "UNSUPPORTED_CURRENCY",
            );
        }
    };
    let state = mutation.state();
    if state.get(&account).and_then(|record| currency_of(record)) != Some(currency.as_str()) {
        return failed(
            &["creditInput", "creditAmount", "currencyCode"],
            "The currency must be the store credit account's currency",
            "MISMATCHING_CURRENCY",
        );
    }
    if balance(state, &account) + amount >= CREDIT_LIMIT {
        return failed(
            &amount_path,
            "The operation would cause the account's credit limit to be exceeded",
            "CREDIT_LIMIT_EXCEEDED",
        );
    }
    let notify = input.get("notify").and_then(Value::as_bool) == Some(true);
    let email = owner_email(state, &account);
    let state = mutation.state_mut();
    let transaction = credit(
        state,
        &account,
        amount,
        expires_at.as_deref(),
        "ADJUSTMENT",
        None,
    );
    if notify && let Some(email) = email {
        let shop = state
            .shop()
            .and_then(|shop| text(shop, "name"))
            .unwrap_or_default()
            .to_string();
        let balance = value::decimal(balance(state, &account));
        outbox::record(
            state,
            Entry::email("store_credit_issued", &email)
                .subject(format!("You received store credit from {shop}"))
                .body(format!(
                    "{} {currency} of store credit was added to your account: your balance is {balance} {currency}.",
                    value::decimal(amount)
                ))
                .about(&transaction),
        );
    }
    Ok(payload(
        json!({"storeCreditAccountTransaction": transaction}),
        &[],
    ))
}

/// `storeCreditAccountDebit`.
fn debit_mutation(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"storeCreditAccountTransaction": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = model::input_of(ctx.arg("debitInput"));
    let (amount, currency) = money(input.get("debitAmount"));
    let currency = currency.unwrap_or_else(|| model::currency(mutation.state()));
    let amount_path = ["debitInput", "debitAmount", "amount"];
    if amount <= 0 {
        return failed(
            &amount_path,
            "A positive amount must be used to debit a store credit account",
            "NEGATIVE_OR_ZERO_AMOUNT",
        );
    }
    let account = match account_of(mutation.state_mut(), &id, &currency, false) {
        Ok(account) => account,
        Err(_) => {
            return failed(
                &["id"],
                "Store credit account does not exist",
                "ACCOUNT_NOT_FOUND",
            );
        }
    };
    let state = mutation.state();
    if state.get(&account).and_then(|record| currency_of(record)) != Some(currency.as_str()) {
        return failed(
            &["debitInput", "debitAmount", "currencyCode"],
            "The currency must be the store credit account's currency",
            "MISMATCHING_CURRENCY",
        );
    }
    let state = mutation.state_mut();
    match debit(state, &account, amount, "ADJUSTMENT", None) {
        Some(transaction) => Ok(payload(
            json!({"storeCreditAccountTransaction": transaction}),
            &[],
        )),
        None => failed(&amount_path, NOT_ENOUGH, "INSUFFICIENT_FUNDS"),
    }
}
