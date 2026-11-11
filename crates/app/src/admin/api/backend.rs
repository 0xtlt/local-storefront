//! Whether the app's backend talks to lsf. A backend started without `LSF_URL` (or without the
//! fetch function of "Point the backend at lsf" in `docs/admin.md`) sends its token exchange and
//! its Admin API calls to Shopify: lsf never sees them, and sees the pages of its frame fail
//! instead (a `500` when the token exchange fails, the status of Shopify's answer when an
//! Admin API call does). The frame's proxy reports those failures here ([`BackendWatch::failed`]),
//! and lsf's admin shows the hint as a banner.

use std::sync::Mutex;

/// What lsf has seen of the backend since it started.
#[derive(Debug, Default)]
pub struct BackendWatch {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Token exchanges and Admin API requests received.
    calls: u64,
    /// What lsf's admin says while the backend seems to bypass lsf.
    hint: Option<String>,
}

impl BackendWatch {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A token exchange or an Admin API request arrived: the backend talks to lsf.
    pub fn called(&self) {
        let mut state = self.state();
        state.calls += 1;
        state.hint = None;
    }

    /// How many token exchanges and Admin API requests lsf has received.
    pub fn calls(&self) -> u64 {
        self.state().calls
    }

    /// A request of the app's frame that carried a session token (`id_token`, or
    /// `Authorization: Bearer`) was answered `status`. When it failed and lsf has received
    /// nothing from the backend yet, the backend likely sends its calls to Shopify: answers the
    /// hint the first time, for the terminal. `lsf` is lsf's address, for `LSF_URL`.
    pub fn failed(&self, path: &str, status: u16, lsf: &str) -> Option<String> {
        if !is_failure(status) {
            return None;
        }
        let mut state = self.state();
        if state.calls > 0 || state.hint.is_some() {
            return None;
        }
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let hint = format!(
            "The app's backend answered {status} to {path}, and lsf has received no request from it: no token exchange, no Admin API call. A backend started without LSF_URL sends them to Shopify. Start it with LSF_URL={lsf}, and check that app/shopify.server.ts replaces the fetch function (setAbstractFetchFunc): \"Point the backend at lsf\" in docs/admin.md."
        );
        state.hint = Some(hint.clone());
        Some(hint)
    }

    /// The hint lsf's admin shows, while the backend has sent nothing to lsf since a request of
    /// the frame failed.
    pub fn hint(&self) -> Option<String> {
        self.state().hint.clone()
    }
}

/// Whether an answer of the backend is a failure that a bypassed lsf explains: an error, but not
/// the `401` of the library's own flows (a session token to renew, a reauthorization) nor the
/// `410` it answers bots.
fn is_failure(status: u16) -> bool {
    status >= 400 && status != 401 && status != 410
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSF: &str = "http://127.0.0.1:9292";

    #[test]
    fn a_failure_before_any_call_names_lsf_url_once() {
        let watch = BackendWatch::default();
        let hint = watch
            .failed("/app?embedded=1&id_token=abc", 500, LSF)
            .expect("a hint");
        assert!(hint.contains("answered 500 to /app,"), "{hint}");
        assert!(hint.contains("LSF_URL=http://127.0.0.1:9292"), "{hint}");
        assert!(hint.contains("setAbstractFetchFunc"), "{hint}");
        assert!(!hint.contains("id_token"), "{hint}");
        assert_eq!(watch.hint(), Some(hint));
        // Said once in the terminal; the admin keeps showing it.
        assert_eq!(watch.failed("/app.data", 404, LSF), None);
        assert!(watch.hint().is_some());
    }

    #[test]
    fn a_call_of_the_backend_clears_the_hint_and_silences_it() {
        let watch = BackendWatch::default();
        assert!(watch.failed("/app", 500, LSF).is_some());
        watch.called();
        assert_eq!(watch.hint(), None);
        assert_eq!(watch.calls(), 1);
        assert_eq!(watch.failed("/app", 500, LSF), None);
        assert_eq!(watch.hint(), None);
    }

    #[test]
    fn the_librarys_own_flows_and_successes_are_not_failures() {
        let watch = BackendWatch::default();
        for status in [200, 302, 401, 410] {
            assert_eq!(watch.failed("/app", status, LSF), None, "{status}");
        }
        assert_eq!(watch.hint(), None);
    }
}
