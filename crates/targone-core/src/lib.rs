//! Read-only core of Targone: discovery of Cargo target directories, layout
//! probing, metadata-only enumeration, and identity-recency classification.
//!
//! Everything in this crate is non-destructive. The classification follows the
//! fail-open invariant (analysis F-060): anything not positively identified as
//! superseded is kept.

pub mod discover;
pub mod layout;
pub mod scan;
pub mod unit;

pub use discover::{discover, TargetDir};
pub use layout::ProfileLayout;
pub use scan::{scan_target_dir, PoolStats, ProfileReport, TargetReport, Tier, TierEstimate};
