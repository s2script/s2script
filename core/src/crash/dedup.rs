//! Stack-signature dedup + rate limiting (spec §6.3): a per-frame thrower must not spam the
//! pipeline. Orthogonal to degrade-per-descriptor — error_count/auto-disable is untouched (D-2).
use std::collections::HashMap;

pub fn fnv1a64(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        // Part separator so ["ab","c"] != ["a","bc"].
        h ^= 0x1f;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub struct RateLimiter {
    last_report: HashMap<u64, u64>, // sig → unix secs of last report
    per_sig: HashMap<u64, u32>,
    total: u32,
}

impl RateLimiter {
    pub const MIN_INTERVAL_SECS: u64 = 60;
    pub const PER_SIG_CAP: u32 = 5;
    pub const TOTAL_CAP: u32 = 100;

    pub fn new() -> Self {
        RateLimiter { last_report: HashMap::new(), per_sig: HashMap::new(), total: 0 }
    }

    pub fn should_report(&mut self, sig: u64, now_secs: u64) -> bool {
        if self.total >= Self::TOTAL_CAP { return false; }
        let count = *self.per_sig.get(&sig).unwrap_or(&0);
        if count >= Self::PER_SIG_CAP { return false; }
        if let Some(&last) = self.last_report.get(&sig) {
            if now_secs.saturating_sub(last) < Self::MIN_INTERVAL_SECS { return false; }
        }
        self.last_report.insert(sig, now_secs);
        self.per_sig.insert(sig, count + 1);
        self.total += 1;
        true
    }
}

impl Default for RateLimiter { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable_and_part_sensitive() {
        let a = fnv1a64(&["p", "msg", "frame"]);
        assert_eq!(a, fnv1a64(&["p", "msg", "frame"]));
        assert_ne!(a, fnv1a64(&["p", "msg", "other"]));
        assert_ne!(fnv1a64(&["ab", "c"]), fnv1a64(&["a", "bc"]), "part boundaries must matter");
    }

    #[test]
    fn rate_limiter_first_then_interval_then_caps() {
        let mut rl = RateLimiter::new();
        let sig = 42u64;
        assert!(rl.should_report(sig, 1000), "first occurrence always reports");
        assert!(!rl.should_report(sig, 1010), "within 60s: suppressed");
        assert!(rl.should_report(sig, 1061), "after 60s: reports again");
        assert!(rl.should_report(sig, 1200));
        assert!(rl.should_report(sig, 1300));
        assert!(rl.should_report(sig, 1400)); // 5th report
        assert!(!rl.should_report(sig, 2000), "PER_SIG_CAP=5 reached");
        // Different signature unaffected.
        assert!(rl.should_report(7, 2000));
    }

    #[test]
    fn rate_limiter_total_cap() {
        let mut rl = RateLimiter::new();
        let mut reported = 0;
        for sig in 0..200u64 {
            if rl.should_report(sig, 5000) { reported += 1; }
        }
        assert_eq!(reported, RateLimiter::TOTAL_CAP as usize);
    }
}
