//! Admission control: may a new agent start without pushing the machine into commit
//! exhaustion or thrashing? Pure policy; memory facts come from `sys::system_memory`.

const GB: u64 = 1024 * 1024 * 1024;

/// What a new agent is assumed to need when the user gives no `--budget`.
pub const DEFAULT_ESTIMATE: u64 = GB;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SystemMemory {
    pub total_phys: u64,
    pub avail_phys: u64,
    pub commit_total: u64,
    pub commit_limit: u64,
}

impl SystemMemory {
    /// Commit charge still available before allocations start failing.
    pub fn commit_headroom(&self) -> u64 {
        self.commit_limit.saturating_sub(self.commit_total)
    }
}

/// Memory kept free for the OS, the terminal and everything else: 10% of RAM, at least 2 GB.
pub fn default_reserve(total_phys: u64) -> u64 {
    (total_phys / 10).max(2 * GB)
}

/// Memory a new agent may use: the tighter of free RAM and commit headroom, minus the reserve.
/// Free RAM guards against thrashing; commit headroom guards against allocation failures.
pub fn safe_zone(mem: &SystemMemory, reserve: u64) -> u64 {
    mem.avail_phys
        .min(mem.commit_headroom())
        .saturating_sub(reserve)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Admit,
    Refuse { need: u64, safe_zone: u64 },
}

pub fn decide(mem: &SystemMemory, reserve: u64, need: u64) -> Admission {
    let zone = safe_zone(mem, reserve);
    if need <= zone {
        Admission::Admit
    } else {
        Admission::Refuse {
            need,
            safe_zone: zone,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(avail: u64, commit_total: u64, commit_limit: u64) -> SystemMemory {
        SystemMemory {
            total_phys: 16 * GB,
            avail_phys: avail,
            commit_total,
            commit_limit,
        }
    }

    #[test]
    fn reserve_is_ten_percent_with_floor() {
        assert_eq!(default_reserve(16 * GB), 2 * GB);
        assert_eq!(default_reserve(64 * GB), 64 * GB / 10);
    }

    #[test]
    fn safe_zone_uses_tighter_of_ram_and_commit() {
        // Plenty of free RAM, but commit nearly exhausted (the Orca #16211 shape).
        let m = mem(8 * GB, 22 * GB, 24 * GB);
        assert_eq!(safe_zone(&m, GB), GB);
        // Commit fine, RAM tight.
        let m = mem(3 * GB, 10 * GB, 24 * GB);
        assert_eq!(safe_zone(&m, GB), 2 * GB);
    }

    #[test]
    fn safe_zone_never_underflows() {
        let m = mem(GB, 23 * GB, 24 * GB);
        assert_eq!(safe_zone(&m, 2 * GB), 0);
    }

    #[test]
    fn admits_or_refuses() {
        let m = mem(10 * GB, 10 * GB, 24 * GB);
        assert_eq!(decide(&m, 2 * GB, 3 * GB), Admission::Admit);
        assert_eq!(
            decide(&m, 2 * GB, 9 * GB),
            Admission::Refuse {
                need: 9 * GB,
                safe_zone: 8 * GB
            }
        );
    }
}
