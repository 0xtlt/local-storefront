//! The outbox of a shop: what Shopify would have sent to the outside world (an email, a
//! WhatsApp message, the processing of a data erasure) and lsf records instead of sending. lsf
//! never sends anything; a developer reads what would have gone out in `GET /__lsf/admin`
//! (`shops[].outbox`), oldest first.
//!
//! An entry is `{at, channel, to, what, about}`: when (the shop's clock), the channel (`email`,
//! `whatsapp`, `data_erasure`), the address or the customer it was for, what it was, and the
//! record it is about. The outbox is a memo of the shop: a mutation that fails takes its entries
//! back, a reset empties it, and it is not store data.

use serde_json::{Value, json};

use crate::state::ShopState;

const MEMO: &str = "lsf/outbox";

/// The most entries kept: the oldest go first.
const KEPT: usize = 500;

/// Records what Shopify would have sent.
pub fn record(state: &mut ShopState, channel: &str, to: &str, what: &str, about: &str) {
    let mut entries = entries(state);
    entries.push(json!({
        "at": state.timestamp(),
        "channel": channel,
        "to": to,
        "what": what,
        "about": about,
    }));
    if entries.len() > KEPT {
        let extra = entries.len() - KEPT;
        entries.drain(..extra);
    }
    state.set_memo(MEMO, Value::Array(entries));
}

/// What the shop would have sent, oldest first.
pub fn entries(state: &ShopState) -> Vec<Value> {
    state
        .memo(MEMO)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
