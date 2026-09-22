//! Platform-agnostic half of CodeNotch: the domain model, settings, the
//! provider adapters and the polling loop that drives them.
//!
//! Deliberately free of any Tauri or GUI dependency so it builds and tests on
//! any host, which is what keeps the adapter parsers covered by fast unit tests.

pub mod adapters;
pub mod collector;
pub mod config;
pub mod estimate;
pub mod layout;
pub mod ledger;
pub mod model;
pub mod prices;
pub mod secrets;
pub mod sqlite;

pub use collector::{Alert, Collector};
pub use config::Config;
pub use layout::{place, Placement, WorkArea};
pub use model::{Activity, Health, ProviderId, ProviderSnapshot, Session, Telemetry, UsageWindow};
