//! The outbox of a shop: what Shopify would have sent to the outside world (an email to a buyer,
//! a shipping label bought from a carrier, a Flow trigger, a message to a POS device, the
//! processing of a data erasure), which lsf records instead of sending. A local tool must never
//! reach a real inbox or a real service, so every family writes here, and a test reads it:
//! `GET /__lsf/admin/outbox?shop=&channel=` (`DELETE` forgets it), `GET /__lsf/admin/emails`
//! (the same, `channel=email`), and each shop's `outbox` in `GET /__lsf/admin`.
//!
//! An entry is `{id, at, channel, to, what, about}`, with `subject` and `body` for an email (the
//! words of the other channels in `body`), and `data` for what else would have gone out (a
//! label's carrier and tracking, a Flow trigger's properties). `what` is a stable name a test
//! matches (`payment_reminder`, `shipping_label`, `flow_trigger`): the words are lsf's, as
//! shopify.dev does not publish the text of Shopify's notification templates.
//!
//! The outbox is a memo of the shop ([`ShopState::set_memo`]): a mutation that fails takes its
//! entries back with its other changes, a reset empties it, and the store sync never sees it.

use serde_json::{Map, Value, json};

use crate::state::ShopState;

/// The memo that holds the outbox: `{"last": <the last id given>, "entries": [...]}`, oldest
/// first. The last id outlives a `DELETE`, so that an id is never given twice.
const MEMO: &str = "lsf/outbox";

/// The most entries an outbox keeps: the oldest go first, so that a long run does not grow the
/// shop without end.
pub const KEPT: usize = 1000;

/// Where an entry would have gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// An email to a buyer, a customer or the merchant.
    Email,
    /// A carrier, for what lsf would have bought from it (a shipping label).
    Carrier,
    /// Shopify Flow, for a trigger of `flowTriggerReceive`.
    Flow,
    /// A device running Shopify POS (a payment session opened, a drawer assigned).
    Pos,
    /// Shopify's processing of a customer's data erasure.
    DataErasure,
}

impl Channel {
    /// Every channel, as `?channel=` names them.
    pub const ALL: [Channel; 5] = [
        Channel::Email,
        Channel::Carrier,
        Channel::Flow,
        Channel::Pos,
        Channel::DataErasure,
    ];

    /// The name of the channel in an entry and in `?channel=`.
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Email => "email",
            Channel::Carrier => "carrier",
            Channel::Flow => "flow",
            Channel::Pos => "pos",
            Channel::DataErasure => "data_erasure",
        }
    }

    /// The channel of a name, `None` for one lsf does not have.
    pub fn parse(name: &str) -> Option<Channel> {
        Channel::ALL
            .into_iter()
            .find(|channel| channel.as_str() == name)
    }
}

/// What lsf records as sent: made with [`Entry::new`] or [`Entry::email`], then the builder
/// methods, and written by [`record`].
#[derive(Clone, Debug)]
pub struct Entry {
    channel: Channel,
    what: String,
    to: Option<String>,
    about: Option<String>,
    subject: Option<String>,
    body: Option<String>,
    data: Option<Value>,
}

impl Entry {
    /// An entry of `channel`: `what` is the stable name a test matches.
    pub fn new(channel: Channel, what: impl Into<String>) -> Entry {
        Entry {
            channel,
            what: what.into(),
            to: None,
            about: None,
            subject: None,
            body: None,
            data: None,
        }
    }

    /// An email: `what` is the name of Shopify's notification (`payment_reminder`,
    /// `draft_order_invoice`, `gift_card_created`, …), `to` the address it went to.
    pub fn email(what: impl Into<String>, to: &str) -> Entry {
        Entry::new(Channel::Email, what).to(to)
    }

    /// Who or what it went to: an address, a carrier, a Flow trigger's handle, a device.
    pub fn to(mut self, to: impl Into<String>) -> Entry {
        self.to = Some(to.into());
        self
    }

    /// The record it is about (an order, a payment schedule, a gift card, a customer).
    pub fn about(mut self, about: impl Into<String>) -> Entry {
        self.about = Some(about.into());
        self
    }

    /// The subject of an email.
    pub fn subject(mut self, subject: impl Into<String>) -> Entry {
        self.subject = Some(subject.into());
        self
    }

    /// The text of an email, or the words of another channel's entry, in a few lines.
    pub fn body(mut self, body: impl Into<String>) -> Entry {
        self.body = Some(body.into());
        self
    }

    /// What else would have gone out, as it would have.
    pub fn data(mut self, data: Value) -> Entry {
        self.data = Some(data);
        self
    }
}

/// Records an entry as sent now, by the shop's clock. Returns its id: `1` for the shop's first,
/// then one more each time, a `DELETE` included.
pub fn record(state: &mut ShopState, entry: Entry) -> u64 {
    let (last, mut entries) = stored(state);
    let id = last + 1;
    let mut written = Map::new();
    written.insert("id".into(), json!(id));
    written.insert("at".into(), json!(state.timestamp()));
    written.insert("channel".into(), json!(entry.channel.as_str()));
    written.insert("to".into(), json!(entry.to));
    written.insert("what".into(), json!(entry.what));
    written.insert("about".into(), json!(entry.about));
    for (name, value) in [
        ("subject", entry.subject.map(Value::String)),
        ("body", entry.body.map(Value::String)),
        ("data", entry.data),
    ] {
        if let Some(value) = value {
            written.insert(name.into(), value);
        }
    }
    entries.push(Value::Object(written));
    if entries.len() > KEPT {
        let excess = entries.len() - KEPT;
        entries.drain(..excess);
    }
    store(state, id, entries);
    id
}

/// The entries of the shop, oldest first: those of `channel`, or all of them.
pub fn entries(state: &ShopState, channel: Option<Channel>) -> Vec<Value> {
    let (_, entries) = stored(state);
    match channel {
        None => entries,
        Some(channel) => entries
            .into_iter()
            .filter(|entry| is_of(entry, channel))
            .collect(),
    }
}

/// Forgets the entries of `channel`, or all of them. Returns how many there were.
pub fn clear(state: &mut ShopState, channel: Option<Channel>) -> usize {
    let (last, entries) = stored(state);
    let before = entries.len();
    let kept: Vec<Value> = match channel {
        None => Vec::new(),
        Some(channel) => entries
            .into_iter()
            .filter(|entry| !is_of(entry, channel))
            .collect(),
    };
    let cleared = before - kept.len();
    if cleared > 0 {
        store(state, last, kept);
    }
    cleared
}

fn is_of(entry: &Value, channel: Channel) -> bool {
    entry.get("channel").and_then(Value::as_str) == Some(channel.as_str())
}

/// The last id given and the entries, from the memo.
fn stored(state: &ShopState) -> (u64, Vec<Value>) {
    let Some(memo) = state.memo(MEMO) else {
        return (0, Vec::new());
    };
    let last = memo.get("last").and_then(Value::as_u64).unwrap_or(0);
    let entries = memo
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (last, entries)
}

fn store(state: &mut ShopState, last: u64, entries: Vec<Value>) {
    state.set_memo(MEMO, json!({"last": last, "entries": entries}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Clock;

    fn shop() -> ShopState {
        ShopState::new("a.myshopify.com", Clock::System)
    }

    #[test]
    fn entries_are_kept_in_order_with_their_channel() {
        let mut state = shop();
        let first = record(
            &mut state,
            Entry::email("payment_reminder", "jane@example.com")
                .about("gid://shopify/PaymentSchedule/1")
                .subject("Payment reminder")
                .body("Pay"),
        );
        let second = record(
            &mut state,
            Entry::new(Channel::Flow, "flow_trigger")
                .to("auction-bid-placed")
                .data(json!({"Amount": "30"})),
        );
        assert_eq!((first, second), (1, 2));
        let all = entries(&state, None);
        assert_eq!(all[0]["channel"], "email");
        assert_eq!(all[0]["what"], "payment_reminder");
        assert_eq!(all[0]["subject"], "Payment reminder");
        assert_eq!(all[1]["about"], Value::Null);
        assert!(all[1].get("subject").is_none(), "{}", all[1]);
        assert_eq!(all[1]["data"]["Amount"], "30");
        let emails = entries(&state, Some(Channel::Email));
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0]["id"], 1);
    }

    #[test]
    fn a_clear_forgets_a_channel_or_all_and_ids_go_on() {
        let mut state = shop();
        record(
            &mut state,
            Entry::email("order_invoice", "jane@example.com"),
        );
        record(
            &mut state,
            Entry::new(Channel::Pos, "payment_session_opened"),
        );
        assert_eq!(clear(&mut state, Some(Channel::Email)), 1);
        assert_eq!(entries(&state, None).len(), 1);
        assert_eq!(clear(&mut state, None), 1);
        assert!(entries(&state, None).is_empty());
        assert_eq!(
            record(&mut state, Entry::email("order_invoice", "x@example.com")),
            3
        );
    }

    #[test]
    fn a_failed_mutation_takes_its_entries_back() {
        let mut state = shop();
        record(
            &mut state,
            Entry::email("order_invoice", "jane@example.com"),
        );
        let savepoint = state.savepoint();
        record(
            &mut state,
            Entry::email("order_invoice", "jane@example.com"),
        );
        state.rollback(savepoint);
        assert_eq!(entries(&state, None).len(), 1);
        assert_eq!(
            record(&mut state, Entry::email("order_invoice", "x@example.com")),
            2
        );
    }

    #[test]
    fn the_outbox_keeps_the_last_entries() {
        let mut state = shop();
        for _ in 0..KEPT + 2 {
            record(&mut state, Entry::new(Channel::Carrier, "shipping_label"));
        }
        let kept = entries(&state, None);
        assert_eq!(kept.len(), KEPT);
        assert_eq!(kept[0]["id"], 3);
    }

    #[test]
    fn channels_are_named_as_entries_name_them() {
        for channel in Channel::ALL {
            assert_eq!(Channel::parse(channel.as_str()), Some(channel));
        }
        assert_eq!(Channel::parse("whatsapp"), None);
    }
}
