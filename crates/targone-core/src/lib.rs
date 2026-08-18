//! Read-only core of Targone: discovery of Cargo target directories, layout
//! probing, metadata-only enumeration, and identity-recency classification.
//!
//! Everything in this crate is non-destructive. The classification follows the
//! fail-open invariant (analysis F-060): anything not positively identified as
//! superseded is kept.

pub mod budget;
pub mod discover;
pub mod fsinfo;
pub mod layout;
pub mod lock;
pub mod registry;
pub mod scan;
pub mod sweep;
pub mod unit;

pub use budget::{parse_size, select_for_budget, BudgetPlan};
pub use discover::{discover, TargetDir};
pub use layout::ProfileLayout;
pub use registry::{Registry, RegistryEntry};
pub use scan::{scan_target_dir, PoolStats, ProfileReport, TargetReport, Tier, TierEstimate};
pub use sweep::{sweep_profile, SweepOutcome};
