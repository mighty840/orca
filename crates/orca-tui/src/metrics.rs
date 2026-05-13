//! Rolling metric history buffers and the human-byte parser used by the
//! services / nodes views. Split out of `state.rs` to keep that file under the
//! workspace's file-size cap.

use std::collections::VecDeque;

/// How many samples to keep in each rolling history buffer. With a 2s
/// refresh tick this gives ~3 minutes of trailing data.
const HISTORY_LEN: usize = 90;

/// Rolling per-service metric history. Reused for nodes too.
#[derive(Debug, Default, Clone)]
pub struct MetricHistory {
    pub cpu: VecDeque<f64>,
    pub mem_bytes: VecDeque<u64>,
    pub disk_used: VecDeque<u64>,
    pub net_rx: VecDeque<u64>,
    pub net_tx: VecDeque<u64>,
}

impl MetricHistory {
    /// Append a new (cpu_percent, memory_bytes) sample, dropping the oldest
    /// once the buffer reaches `HISTORY_LEN`. Used by services and nodes.
    pub fn push_basic(&mut self, cpu: f64, mem_bytes: u64) {
        push_capped(&mut self.cpu, cpu);
        push_capped(&mut self.mem_bytes, mem_bytes);
    }

    /// Extended sample for nodes (also tracks disk + network).
    pub fn push_full(&mut self, cpu: f64, mem_bytes: u64, disk_used: u64, rx: u64, tx: u64) {
        self.push_basic(cpu, mem_bytes);
        push_capped(&mut self.disk_used, disk_used);
        push_capped(&mut self.net_rx, rx);
        push_capped(&mut self.net_tx, tx);
    }
}

fn push_capped<T>(buf: &mut VecDeque<T>, value: T) {
    if buf.len() >= HISTORY_LEN {
        buf.pop_front();
    }
    buf.push_back(value);
}

/// Best-effort parser for `42.5MiB` / `1024Ki` / `1.2G` style strings into
/// raw bytes. Used to convert the string-form `memory_usage` reported by
/// the API into a number we can plot.
pub fn parse_human_bytes(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let (num_part, suffix) = s
        .find(|c: char| c.is_alphabetic())
        .map(|i| (&s[..i], &s[i..]))
        .unwrap_or((s, ""));
    let n: f64 = num_part.parse().unwrap_or(0.0);
    let mult: f64 = match suffix.to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "ki" | "kib" => 1024.0,
        "m" | "mb" | "mi" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gi" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "ti" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (n * mult) as u64
}
