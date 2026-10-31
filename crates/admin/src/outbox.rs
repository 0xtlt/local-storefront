//! The emails lsf records instead of sending them. Shopify emails the buyer when a merchant
//! sends a payment reminder, an invoice, a gift card or a store credit notice; a local tool
//! must never reach a real inbox, so each such email is kept in the shop's outbox, which a test
//! reads through `GET /__lsf/admin/emails` (and forgets with `DELETE`).
//!
//! The outbox is a memo of the shop ([`ShopState::set_memo`]): a mutation that fails takes its
//! emails back with its other changes, a reset empties it, and the store sync never sees it.
//! The subject and the text are lsf's wording of what Shopify's notification templates say;
//! shopify.dev does not publish the templates' text.

use serde_json::{Value, json};

use crate::state::ShopState;

/// The memo that holds the outbox: an array of emails, oldest first.
const OUTBOX: &str = "lsf/outbox";

/// The most emails an outbox keeps: older ones are dropped, so that a long run does not grow
/// the shop without end.
const MAX_EMAILS: usize = 1000;

/// An email lsf records as sent.
#[derive(Clone, Debug, Default)]
pub struct Email<'a> {
    /// What Shopify's notification is called (`payment_reminder`, `draft_order_invoice`,
    /// `gift_card_created`, …): what a test looks for.
    pub template: &'a str,
    /// The address it went to.
    pub to: &'a str,
    pub subject: String,
    /// The text of the email, in a few lines.
    pub body: String,
    /// The record it is about (an order, a payment schedule, a gift card).
    pub resource: Option<&'a str>,
}

/// Records an email as sent now, by the shop's clock. Returns its id (`1` for the first).
pub fn send(state: &mut ShopState, email: &Email) -> u64 {
    let mut emails = emails(state);
    let id = emails
        .last()
        .and_then(|last| last.get("id"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + 1;
    emails.push(json!({
        "id": id,
        "template": email.template,
        "to": email.to,
        "subject": email.subject,
        "body": email.body,
        "resource": email.resource,
        "sentAt": state.timestamp(),
    }));
    if emails.len() > MAX_EMAILS {
        let excess = emails.len() - MAX_EMAILS;
        emails.drain(..excess);
    }
    state.set_memo(OUTBOX, Value::Array(emails));
    id
}

/// The emails of the shop, oldest first.
pub fn emails(state: &ShopState) -> Vec<Value> {
    state
        .memo(OUTBOX)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Forgets the emails of the shop. Returns how many there were.
pub fn clear(state: &mut ShopState) -> usize {
    let count = emails(state).len();
    if count > 0 {
        state.set_memo(OUTBOX, json!([]));
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Clock;

    #[test]
    fn emails_are_kept_in_order_and_forgotten() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        let first = send(
            &mut state,
            &Email {
                template: "payment_reminder",
                to: "jane@example.com",
                subject: "Payment reminder".into(),
                body: "Pay".into(),
                resource: Some("gid://shopify/PaymentSchedule/1"),
            },
        );
        let second = send(
            &mut state,
            &Email {
                template: "draft_order_invoice",
                to: "jane@example.com",
                ..Email::default()
            },
        );
        assert_eq!((first, second), (1, 2));
        let sent = emails(&state);
        assert_eq!(sent[0]["template"], "payment_reminder");
        assert_eq!(sent[1]["resource"], Value::Null);
        assert_eq!(clear(&mut state), 2);
        assert!(emails(&state).is_empty());
    }
}
