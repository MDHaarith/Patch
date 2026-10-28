//! Hybrid logical clock.
//!
//! A [`Stamp`] is `WWWWWWWWWWWWWWW-CCCC-node`: wall-clock milliseconds, a
//! counter for writes within the same millisecond and the writing node's id.
//! The fixed-width encoding makes plain string comparison equal to causal
//! ordering, so stamps can be compared without parsing.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_FUTURE_SKEW_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Stamp(pub String);

impl Stamp {
    /// Stamp used for built-in seed records. Any real write beats it.
    pub fn seed() -> Stamp {
        Stamp("000000000000000-0000-seed".into())
    }

    pub fn new(wall: u64, counter: u32, node: &str) -> Stamp {
        Stamp(format!("{wall:015}-{counter:04}-{node}"))
    }

    pub fn parts(&self) -> Option<(u64, u32)> {
        let mut it = self.0.splitn(3, '-');
        let wall = it.next()?.parse().ok()?;
        let counter = it.next()?.parse().ok()?;
        Some((wall, counter))
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct Clock {
    pub node: String,
    wall: u64,
    counter: u32,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

impl Clock {
    pub fn new(node: impl Into<String>) -> Clock {
        Clock { node: node.into(), wall: 0, counter: 0 }
    }

    pub fn restore(node: impl Into<String>, wall: u64, counter: u32) -> Clock {
        Clock { node: node.into(), wall, counter }
    }

    pub fn state(&self) -> (u64, u32) {
        (self.wall, self.counter)
    }

    /// Stamp for a local write happening at `now` (milliseconds).
    pub fn tick_at(&mut self, now: u64) -> Stamp {
        if now > self.wall {
            self.wall = now;
            self.counter = 0;
        } else if self.counter >= 9999 {
            self.wall = self.wall.saturating_add(1);
            self.counter = 0;
        } else {
            self.counter += 1;
        }
        Stamp::new(self.wall, self.counter, &self.node)
    }

    pub fn tick(&mut self) -> Stamp {
        self.tick_at(now_ms())
    }

    /// Move the clock past a stamp received from another node so that our
    /// next local write orders after everything we have seen.
    pub fn observe(&mut self, stamp: &Stamp) {
        if let Some(remote) = stamp.parts() {
            if remote.0 <= now_ms().saturating_add(MAX_FUTURE_SKEW_MS) && remote > (self.wall, self.counter) {
                self.wall = remote.0;
                self.counter = remote.1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_order_lexicographically() {
        let mut c = Clock::new("a");
        let s1 = c.tick_at(5);
        let s2 = c.tick_at(5);
        let s3 = c.tick_at(4); // wall clock went backwards
        let s4 = c.tick_at(1_000_000_000_000);
        assert!(s1 < s2 && s2 < s3 && s3 < s4);
        assert!(Stamp::seed() < s1);
    }

    #[test]
    fn observe_ignores_far_future_stamp() {
        let mut clock = Clock::new("local");
        clock.tick_at(100);
        clock.observe(&Stamp::new(
            now_ms() + MAX_FUTURE_SKEW_MS + 60_000,
            0,
            "remote",
        ));
        assert_eq!(clock.state(), (100, 0));
    }

    #[test]
    fn maximum_remote_counter_advances_wall_time() {
        let mut clock = Clock::new("local");
        let remote = Stamp::new(now_ms(), u32::MAX, "remote");
        clock.observe(&remote);
        let local = clock.tick_at(0);
        assert!(local > remote);
        assert_eq!(local.parts(), Some((remote.parts().unwrap().0 + 1, 0)));
        assert!(clock.tick_at(0) > local);
    }

    #[test]
    fn observe_moves_clock_forward() {
        let mut a = Clock::new("a");
        let mut b = Clock::new("b");
        let remote = b.tick_at(10_000);
        a.observe(&remote);
        let local = a.tick_at(5);
        assert!(local > remote);
        assert_eq!(remote.parts(), Some((10_000, 0)));
    }
}
