//! Patch's own CPU and memory use, read from `/proc/self` (Linux and Android).

use std::time::Instant;

// shortcut: assumes USER_HZ = 100 (true on Linux and Android); read sysconf(_SC_CLK_TCK) if a port differs.
const TICKS_PER_SEC: f64 = 100.0;

/// utime + stime in clock ticks from a `/proc/<pid>/stat` line. The command
/// name may contain spaces and parentheses, so fields count from the last `)`.
fn cpu_ticks(stat: &str) -> Option<u64> {
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

fn rss_kb(status: &str) -> Option<u64> {
    status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.split_whitespace().next()?.parse().ok()
}

/// Remembers the previous sample so CPU % covers the time between refreshes.
#[derive(Default)]
pub struct Usage {
    last: Option<(Instant, u64)>,
}

impl Usage {
    /// `CPU 0.4% · 52 MB · 3m 12s CPU since launch`, or `None` without `/proc`.
    pub fn sample(&mut self) -> Option<String> {
        let ticks = cpu_ticks(&std::fs::read_to_string("/proc/self/stat").ok()?)?;
        let rss = rss_kb(&std::fs::read_to_string("/proc/self/status").ok()?)?;
        let now = Instant::now();
        let cpu = match self.last.replace((now, ticks)) {
            Some((then, before)) => {
                let secs = now.duration_since(then).as_secs_f64().max(0.001);
                format!("{:.1}%", ticks.saturating_sub(before) as f64 / TICKS_PER_SEC / secs * 100.0)
            }
            None => "…".into(),
        };
        let total = (ticks as f64 / TICKS_PER_SEC) as u64;
        Some(format!("CPU {cpu} · {} MB · {}m {}s CPU since launch", rss / 1024, total / 60, total % 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        let stat = "4242 (patch (ui) x) S 1 4242 4242 0 -1 4194560 900 0 0 0 1234 56 0 0 20 0 9 0 100 0 0";
        assert_eq!(cpu_ticks(stat), Some(1290));
        assert_eq!(rss_kb("Name:\tpatch\nVmRSS:\t   53248 kB\nThreads: 9\n"), Some(53248));
        assert_eq!(rss_kb("Name:\tpatch\n"), None);
    }
}
