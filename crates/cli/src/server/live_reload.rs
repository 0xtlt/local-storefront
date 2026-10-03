//! Live reload: the open pages are told when the theme or the store data change.
//!
//! A page listens on a WebSocket at `/__lsf/livereload`. The same URL without the upgrade
//! answers with the token of the files, for the pages that cannot open a socket and ask
//! instead. Either way a page reloads when the token is no longer the first one it got.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use super::ServerState;

/// The script injected into pages by `--live-reload`.
pub const SCRIPT: &str = concat!(
    "<script data-lsf-live-reload>\n",
    include_str!("../../assets/live-reload.js"),
    "</script>"
);

/// How often the files are looked at while a page listens.
const INTERVAL: Duration = Duration::from_millis(100);

/// How long the files have to hold still before the pages are told that they changed. A save
/// or a build writes more than once: the pages reload once, when it is over.
const SETTLE: Duration = Duration::from_millis(50);

/// How many times the files are given `SETTLE` to hold still. Files that never do (a log
/// written next to the data) must not keep a change from the pages forever.
const SETTLE_ATTEMPTS: usize = 20;

/// The token of the files, as last seen for the pages that listen.
pub struct Changes {
    token: watch::Sender<u64>,
    /// Whether the task that looks at the files has been started.
    watching: AtomicBool,
}

impl Changes {
    pub fn new() -> Changes {
        Changes {
            token: watch::channel(0).0,
            watching: AtomicBool::new(false),
        }
    }

    /// The token of the files as they are now.
    async fn look(state: &Arc<ServerState>) -> Option<u64> {
        let files = state.clone();
        // Walking the directories is file system work: off the async workers.
        tokio::task::spawn_blocking(move || files.observe_changes())
            .await
            .ok()
    }

    /// Tells the pages that listen, when the token is a new one.
    fn publish(&self, token: u64) {
        self.token
            .send_if_modified(|current| std::mem::replace(current, token) != token);
    }
}

/// Looks at the files for as long as the server runs, whenever a page listens.
async fn watch(state: Arc<ServerState>) {
    let changes = &state.changes;
    let mut ticks = tokio::time::interval(INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        if changes.token.receiver_count() == 0 {
            continue;
        }
        let Some(mut token) = Changes::look(&state).await else {
            continue;
        };
        if token == *changes.token.borrow() {
            continue;
        }
        for _ in 0..SETTLE_ATTEMPTS {
            tokio::time::sleep(SETTLE).await;
            match Changes::look(&state).await {
                Some(next) if next != token => token = next,
                _ => break,
            }
        }
        changes.publish(token);
    }
}

/// Serves one page: the token of the files now, then every token they take.
pub async fn serve(mut socket: WebSocket, state: Arc<ServerState>) {
    let mut tokens = state.changes.token.subscribe();
    if !state.changes.watching.swap(true, Ordering::Relaxed) {
        tokio::spawn(watch(state.clone()));
    }
    // What the files are now, for a page that has not been told anything yet.
    if let Some(token) = Changes::look(&state).await {
        state.changes.publish(token);
    }
    loop {
        // The latest token, also when a newer one came while this one was being sent.
        let token = *tokens.borrow_and_update();
        if socket
            .send(Message::Text(token.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        loop {
            tokio::select! {
                changed = tokens.changed() => match changed {
                    Ok(()) => break,
                    Err(_) => return,
                },
                // Nothing is expected from the page, except that it leaves.
                message = socket.recv() => match message {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                    Some(Ok(_)) => {}
                },
            }
        }
    }
}
