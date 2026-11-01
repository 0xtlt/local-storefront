//! The cost bucket of each shop and app: Shopify's leaky bucket of the GraphQL Admin API.
//!
//! A bucket holds up to `maximumAvailable` points and refills at `restoreRate` points a second.
//! A query runs only if the bucket has its requested cost, which it takes; once it ran, the
//! bucket gets back what it did not cost (requested minus actual). A query the bucket cannot
//! pay for is refused with [`throttled`], and takes nothing. That is shopify.dev's "GraphQL Admin
//! API rate limits" (read in 2026-10): limits per app and store, restore rates by plan.
//!
//! | Plan | `restoreRate` | `maximumAvailable` |
//! |---|---|---|
//! | Standard (the default) | 100 | 2000 |
//! | Advanced | 200 | 4000 |
//! | Plus | 1000 | 20000 |
//! | Enterprise (Commerce Components) | 2000 | 40000 |
//!
//! shopify.dev publishes the restore rates, not the bucket sizes: lsf gives each plan a bucket of
//! twenty seconds of restore, which is what the stores of the current plans report
//! (`maximumAvailable: 2000.0` with `restoreRate: 100.0`). The older plans of shopify.dev's
//! examples (1000 and 50) are [`Plan::LEGACY`].
//!
//! The clock is injected ([`Clock`]), so that a test can empty a bucket and wait without waiting.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::graphql::Error;

/// The size and the restore rate of a bucket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// `maximumAvailable`: what a full bucket holds.
    pub maximum_available: f64,
    /// `restoreRate`: the points the bucket gets back each second.
    pub restore_rate: f64,
}

impl Plan {
    pub const STANDARD: Plan = Plan {
        maximum_available: 2000.0,
        restore_rate: 100.0,
    };
    pub const ADVANCED: Plan = Plan {
        maximum_available: 4000.0,
        restore_rate: 200.0,
    };
    pub const PLUS: Plan = Plan {
        maximum_available: 20000.0,
        restore_rate: 1000.0,
    };
    pub const ENTERPRISE: Plan = Plan {
        maximum_available: 40000.0,
        restore_rate: 2000.0,
    };
    /// The bucket of shopify.dev's examples, before the current plans.
    pub const LEGACY: Plan = Plan {
        maximum_available: 1000.0,
        restore_rate: 50.0,
    };

    /// The plan of a name, as a store's plan is written (`basic` and `shopify` are Standard;
    /// `shopify_plus` is Plus).
    pub fn from_name(name: &str) -> Option<Plan> {
        let name = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Some(match name.as_str() {
            "standard" | "basic" | "shopify" | "starter" | "development" | "partner_test"
            | "trial" => Plan::STANDARD,
            "advanced" | "advanced_shopify" => Plan::ADVANCED,
            "plus" | "shopify_plus" => Plan::PLUS,
            "enterprise" | "commerce_components" | "shopify_for_enterprise" => Plan::ENTERPRISE,
            "legacy" => Plan::LEGACY,
            _ => return None,
        })
    }

    /// The status of a full bucket.
    pub fn full(self) -> ThrottleStatus {
        ThrottleStatus {
            maximum_available: self.maximum_available,
            currently_available: self.maximum_available,
            restore_rate: self.restore_rate,
        }
    }
}

impl Default for Plan {
    fn default() -> Plan {
        Plan::STANDARD
    }
}

/// `throttleStatus`: the state of a bucket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThrottleStatus {
    pub maximum_available: f64,
    pub currently_available: f64,
    pub restore_rate: f64,
}

impl ThrottleStatus {
    /// As Shopify writes it: the size and the rate as floats (`1000.0`), what is available as
    /// a whole number of points.
    pub fn to_json(&self) -> Value {
        json!({
            "maximumAvailable": self.maximum_available,
            "currentlyAvailable": self.currently_available.max(0.0).floor() as u64,
            "restoreRate": self.restore_rate,
        })
    }
}

/// Shopify's error for a request its bucket cannot pay for (shopify.dev's response codes page,
/// and the answers of the Admin API): `{"message": "Throttled", "extensions": {"code":
/// "THROTTLED", "documentation": ...}}`. The answer has no `data`, and `actualQueryCost: null`.
pub fn throttled() -> Error {
    Error::new("Throttled")
        .with_extension("code", "THROTTLED")
        .with_extension("documentation", "https://shopify.dev/api/usage/rate-limits")
}

/// The key of the bucket of an app on a shop.
pub fn key(shop: &str, app: &str) -> String {
    format!("{shop} {app}")
}

/// A monotonic clock: the time since some start.
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

/// The clock of the system.
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> SystemClock {
        SystemClock(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A clock that moves only when told to, for tests.
#[derive(Default)]
pub struct ManualClock(AtomicU64);

impl ManualClock {
    pub fn advance(&self, by: Duration) {
        let nanos = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        self.0.fetch_add(nanos, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::SeqCst))
    }
}

impl<C: Clock> Clock for std::sync::Arc<C> {
    fn now(&self) -> Duration {
        self.as_ref().now()
    }
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    available: f64,
    /// When `available` was last brought up to date.
    at: Duration,
}

/// The buckets of every shop and app.
pub struct Throttle {
    plan: Plan,
    enabled: AtomicBool,
    clock: Box<dyn Clock>,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl Default for Throttle {
    fn default() -> Throttle {
        Throttle::new(Plan::default())
    }
}

impl Throttle {
    /// Buckets of a plan, on the clock of the system.
    pub fn new(plan: Plan) -> Throttle {
        Throttle::with_clock(plan, SystemClock::default())
    }

    pub fn with_clock(plan: Plan, clock: impl Clock + 'static) -> Throttle {
        Throttle {
            plan,
            enabled: AtomicBool::new(true),
            clock: Box::new(clock),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn plan(&self) -> Plan {
        self.plan
    }

    /// Turns throttling off (`--admin-throttle off`) or on. Off, every bucket is always full:
    /// nothing is refused, and `throttleStatus` reports a full bucket.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Fills every bucket.
    pub fn reset(&self) {
        self.buckets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Takes the requested cost of a query from a bucket: the status after, or, when the bucket
    /// does not have it, the status as it is (the query must not run).
    pub fn admit(&self, key: &str, requested: u64) -> Result<ThrottleStatus, ThrottleStatus> {
        self.update(key, |bucket| {
            if bucket.available < requested as f64 {
                return false;
            }
            bucket.available -= requested as f64;
            true
        })
    }

    /// Gives a bucket back what a query that ran did not cost.
    pub fn settle(&self, key: &str, requested: u64, actual: u64) -> ThrottleStatus {
        let refund = requested.saturating_sub(actual) as f64;
        let (Ok(status) | Err(status)) = self.update(key, |bucket| {
            bucket.available += refund;
            true
        });
        status
    }

    /// The status of a bucket now.
    pub fn status(&self, key: &str) -> ThrottleStatus {
        let (Ok(status) | Err(status)) = self.update(key, |_| true);
        status
    }

    fn update(
        &self,
        key: &str,
        change: impl FnOnce(&mut Bucket) -> bool,
    ) -> Result<ThrottleStatus, ThrottleStatus> {
        let plan = self.plan();
        if !self.is_enabled() {
            return Ok(plan.full());
        }
        let now = self.clock.now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            available: plan.maximum_available,
            at: now,
        });
        let elapsed = now.saturating_sub(bucket.at).as_secs_f64();
        bucket.available =
            (bucket.available + elapsed * plan.restore_rate).min(plan.maximum_available);
        bucket.at = now;
        let done = change(bucket);
        bucket.available = bucket.available.min(plan.maximum_available);
        let status = ThrottleStatus {
            maximum_available: plan.maximum_available,
            currently_available: bucket.available,
            restore_rate: plan.restore_rate,
        };
        if done { Ok(status) } else { Err(status) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_bucket_leaks_and_refills() {
        let clock = Arc::new(ManualClock::default());
        let throttle = Throttle::with_clock(Plan::LEGACY, clock.clone());
        let key = key("my-shop.myshopify.com", "app");
        // shopify.dev's example: 101 requested, 46 actual, 954 left of 1000.
        let status = throttle.admit(&key, 101).unwrap();
        assert_eq!(status.currently_available, 899.0);
        let status = throttle.settle(&key, 101, 46);
        assert_eq!(
            status.to_json(),
            json!({"maximumAvailable": 1000.0, "currentlyAvailable": 954, "restoreRate": 50.0})
        );
        // Emptied, then refused without taking anything.
        throttle.admit(&key, 954).unwrap();
        let refused = throttle.admit(&key, 10).unwrap_err();
        assert_eq!(refused.currently_available, 0.0);
        // A tenth of a second later, 5 points are back.
        clock.advance(Duration::from_millis(100));
        assert_eq!(throttle.status(&key).currently_available, 5.0);
        assert!(throttle.admit(&key, 10).is_err());
        clock.advance(Duration::from_millis(100));
        assert_eq!(throttle.admit(&key, 10).unwrap().currently_available, 0.0);
        // Never more than full.
        clock.advance(Duration::from_secs(3600));
        assert_eq!(throttle.status(&key).currently_available, 1000.0);
        // Another app on the same shop has its own bucket.
        throttle.admit(&key, 1000).unwrap();
        let other = super::key("my-shop.myshopify.com", "other-app");
        assert_eq!(throttle.status(&other).currently_available, 1000.0);
    }

    #[test]
    fn turned_off_it_refuses_nothing() {
        let throttle = Throttle::with_clock(Plan::STANDARD, ManualClock::default());
        throttle.set_enabled(false);
        for _ in 0..10 {
            assert_eq!(throttle.admit("k", 1000), Ok(Plan::STANDARD.full()));
        }
        throttle.set_enabled(true);
        assert_eq!(
            throttle.admit("k", 1500).unwrap().currently_available,
            500.0
        );
        assert!(throttle.admit("k", 1000).is_err());
    }

    #[test]
    fn plans_by_name() {
        assert_eq!(Plan::from_name("Shopify Plus"), Some(Plan::PLUS));
        assert_eq!(Plan::from_name("basic"), Some(Plan::STANDARD));
        assert_eq!(Plan::from_name("advanced"), Some(Plan::ADVANCED));
        assert_eq!(
            Plan::from_name("commerce-components"),
            Some(Plan::ENTERPRISE)
        );
        assert_eq!(Plan::from_name("gold"), None);
        assert_eq!(Plan::default(), Plan::STANDARD);
    }

    #[test]
    fn the_throttled_error_is_shopifys() {
        assert_eq!(
            throttled().to_json(),
            json!({
                "message": "Throttled",
                "extensions": {"code": "THROTTLED", "documentation": "https://shopify.dev/api/usage/rate-limits"}
            })
        );
    }
}
