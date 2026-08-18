//! Budget selection: which target dirs to sweep so the machine fits under a
//! global byte budget.
//!
//! F-048: the budget is an *ordering and stopping* function over RECLAIMABLE
//! bytes — never a per-project quota (F-001: the distribution is heavy-tail)
//! and never measured over bytes the engine cannot delete (cargo-sweep's
//! `--maxsize` mistake, F-034).

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BudgetPlan {
    /// Total bytes currently held by all managed dirs.
    pub total_bytes: u64,
    /// Bytes that must go to reach the budget.
    pub need_bytes: u64,
    /// Bytes the selected sweeps are expected to free.
    pub planned_bytes: u64,
    /// True when even sweeping everything reclaimable cannot reach the
    /// budget — the caller should say so instead of implying success.
    pub insufficient: bool,
}

/// Select which dirs to sweep, given `(total, reclaimable)` per dir.
/// Returns the indices to sweep (descending reclaimable order) plus the plan
/// summary. An unset budget (`None`) selects every dir with anything to
/// reclaim.
pub fn select_for_budget(dirs: &[(u64, u64)], budget: Option<u64>) -> (Vec<usize>, BudgetPlan) {
    let total_bytes: u64 = dirs.iter().map(|(t, _)| t).sum();
    let mut order: Vec<usize> = (0..dirs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(dirs[i].1));

    let need_bytes = match budget {
        None => u64::MAX,
        Some(b) => total_bytes.saturating_sub(b),
    };
    let mut selected = Vec::new();
    let mut planned_bytes = 0u64;
    for i in order {
        if dirs[i].1 == 0 || (budget.is_some() && planned_bytes >= need_bytes) {
            continue;
        }
        planned_bytes += dirs[i].1;
        selected.push(i);
    }
    let need_bytes = if budget.is_none() {
        planned_bytes
    } else {
        need_bytes
    };
    (
        selected,
        BudgetPlan {
            total_bytes,
            need_bytes,
            planned_bytes,
            insufficient: planned_bytes < need_bytes,
        },
    )
}

/// Parse a human byte size: `"20GB"`, `"1.5 TiB"`, `"500 MB"`, bare bytes.
/// Decimal suffixes (KB/MB/GB/TB) are powers of 1000; binary (KiB/MiB/GiB/
/// TiB) powers of 1024.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num.trim().parse().ok()?;
    if value < 0.0 {
        return None;
    }
    let mult: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1000,
        "mb" => 1000_u64.pow(2),
        "gb" => 1000_u64.pow(3),
        "tb" => 1000_u64.pow(4),
        "kib" => 1024,
        "mib" => 1024_u64.pow(2),
        "gib" => 1024_u64.pow(3),
        "tib" => 1024_u64.pow(4),
        _ => return None,
    };
    Some((value * mult as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("20GB"), Some(20_000_000_000));
        assert_eq!(parse_size("1.5 GiB"), Some(1_610_612_736));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("100 MiB"), Some(104_857_600));
        assert_eq!(parse_size("2TB"), Some(2_000_000_000_000));
        assert_eq!(parse_size("1TiB"), Some(1_099_511_627_776));
        assert_eq!(parse_size("3KB"), Some(3000));
        assert_eq!(parse_size("banana"), None);
        assert_eq!(parse_size("5xb"), None); // unknown unit
        assert_eq!(parse_size("-5GB"), None);
    }

    #[test]
    fn no_budget_selects_everything_reclaimable() {
        let dirs = [(100, 50), (200, 0), (300, 120)];
        let (sel, plan) = select_for_budget(&dirs, None);
        assert_eq!(sel, vec![2, 0]); // descending reclaimable
        assert_eq!(plan.planned_bytes, 170);
        assert!(!plan.insufficient);
    }

    #[test]
    fn budget_stops_once_met() {
        // total 600, budget 480 → need 120; biggest reclaimable first.
        let dirs = [(100, 50), (200, 40), (300, 120)];
        let (sel, plan) = select_for_budget(&dirs, Some(480));
        assert_eq!(sel, vec![2]); // 120 covers the need exactly
        assert_eq!(plan.need_bytes, 120);
        assert_eq!(plan.planned_bytes, 120);
        assert!(!plan.insufficient);
    }

    #[test]
    fn under_budget_selects_nothing() {
        let dirs = [(100, 50), (200, 40)];
        let (sel, plan) = select_for_budget(&dirs, Some(1000));
        assert!(sel.is_empty());
        assert_eq!(plan.need_bytes, 0);
    }

    #[test]
    fn insufficient_budget_is_reported() {
        let dirs = [(1000, 10)];
        let (sel, plan) = select_for_budget(&dirs, Some(100));
        assert_eq!(sel, vec![0]);
        assert!(plan.insufficient);
        assert_eq!(plan.need_bytes, 900);
        assert_eq!(plan.planned_bytes, 10);
    }
}
