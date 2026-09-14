//! Resource sampling for the top monitor bar: system-wide and this app's own
//! CPU%, RAM, and (where available) VRAM.
//!
//! CPU and RAM come from the `sysinfo` crate. VRAM is read from `nvidia-smi`
//! — `sysinfo` exposes no system-wide VRAM API on any platform — so NVIDIA
//! GPU users get real numbers and everyone else reads `n/a`. That subprocess
//! runs **off the main thread** (see [`Sampler::tick`]) so the UI loop never
//! stalls on it.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

/// A per-tick snapshot of resource usage.
#[derive(Debug, Clone, Copy, Default)]
pub struct Resources {
    // System-wide.
    pub sys_cpu: f32,
    pub sys_ram_used: u64,
    pub sys_ram_total: u64,
    pub sys_vram_used: Option<u64>,
    pub sys_vram_total: Option<u64>,
    // This app's own process.
    pub app_cpu: f32,
    pub app_ram: u64,
}

/// Samples resource usage on a 1-second gate.
///
/// Holds a `sysinfo::System`, the last [`Resources`] snapshot, and a
/// non-blocking channel carrying the nvidia-smi VRAM fallback result.
pub struct Sampler {
    system: System,
    last: Resources,
    last_tick: Instant,
    vram_tx: mpsc::Sender<Option<(u64, u64)>>,
    vram_rx: mpsc::Receiver<Option<(u64, u64)>>,
    /// True while an nvidia-smi probe is in flight, so we don't re-spawn.
    vram_pending: bool,
    /// The last VRAM value shown, carried forward while a probe is in flight so
    /// the bar doesn't flicker to `n/a` between probes.
    last_vram: Option<(u64, u64)>,
}

impl Sampler {
    /// Build a sampler and prime it with one full refresh so the first 1s
    /// window is a real measurement. Kicks off the first nvidia-smi probe
    /// immediately so VRAM is available as soon as the first window elapses.
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let (vram_tx, vram_rx) = mpsc::channel();
        spawn_nvidia_smi(vram_tx.clone());
        let last = snapshot(&system, None);
        Self {
            system,
            last,
            last_tick: Instant::now(),
            vram_tx,
            vram_rx,
            vram_pending: true,
            last_vram: None,
        }
    }

    /// Advance the sampler: when the 1s gate elapses, refresh and take a new
    /// snapshot. Always returns the most recent snapshot.
    pub fn tick(&mut self) -> &Resources {
        if self.last_tick.elapsed() < Duration::from_secs(1) {
            return &self.last;
        }
        self.last_tick = Instant::now();
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system.refresh_processes(ProcessesToUpdate::All, true);

        // A finished nvidia-smi probe, if one was in flight.
        if let Ok(result) = self.vram_rx.try_recv() {
            self.vram_pending = false;
            self.last_vram = result;
        }

        // If no probe is in flight, launch one so VRAM stays fresh. While a
        // probe is in flight we keep showing `last_vram` (avoids flicker to
        // `n/a` between probes).
        if !self.vram_pending {
            self.vram_pending = true;
            spawn_nvidia_smi(self.vram_tx.clone());
        }

        self.last = snapshot(&self.system, self.last_vram);
        &self.last
    }
}

/// Take a [`Resources`] snapshot from `system`, with the resolved VRAM value.
fn snapshot(system: &System, vram: Option<(u64, u64)>) -> Resources {
    let pid = Pid::from(std::process::id() as usize);
    let proc = system.process(pid);
    let (sys_vram_used, sys_vram_total) = match vram {
        Some((used, total)) => (Some(used), Some(total)),
        None => (None, None),
    };
    Resources {
        sys_cpu: system.global_cpu_usage(),
        sys_ram_used: system.used_memory(),
        sys_ram_total: system.total_memory(),
        sys_vram_used,
        sys_vram_total,
        app_cpu: proc.map(|p| p.cpu_usage()).unwrap_or(0.0),
        app_ram: proc.map(|p| p.memory()).unwrap_or(0),
    }
}

/// Spawn a background thread that runs `nvidia-smi` and sends the summed
/// (used, total) VRAM in bytes — or `None` if the tool is absent/failed — over
/// `tx`. Never blocks the caller.
fn spawn_nvidia_smi(tx: mpsc::Sender<Option<(u64, u64)>>) {
    std::thread::spawn(move || {
        let result = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=memory.used,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output();
        let vram = match result {
            Ok(output) if output.status.success() => {
                parse_nvidia_smi(&String::from_utf8_lossy(&output.stdout))
            }
            _ => None,
        };
        let _ = tx.send(vram);
    });
}

/// Parse `nvidia-smi --query-gpu=memory.used,memory.total
/// --format=csv,noheader,nounits` output: one `<usedMiB>,<totalMiB>` line per
/// GPU. Sums across GPUs and converts MiB → bytes. Returns `None` if no line
/// parses.
fn parse_nvidia_smi(output: &str) -> Option<(u64, u64)> {
    let mut used = 0u64;
    let mut total = 0u64;
    let mut any = false;
    for line in output.lines() {
        let mut fields = line.split(',');
        let u = match fields.next() {
            Some(s) => s.trim().parse::<u64>().ok(),
            None => continue,
        };
        let t = match fields.next() {
            Some(s) => s.trim().parse::<u64>().ok(),
            None => continue,
        };
        if let (Some(u), Some(t)) = (u, t) {
            used += u * 1024 * 1024;
            total += t * 1024 * 1024;
            any = true;
        }
    }
    if any {
        Some((used, total))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nvidia_smi_sums_two_gpus() {
        // Two GPUs: 1024+2048 MiB used, 8192+8192 MiB total.
        let out = "1024, 8192\n2048, 8192\n";
        let (used, total) = parse_nvidia_smi(out).unwrap();
        assert_eq!(used, (1024 + 2048) * 1024 * 1024);
        assert_eq!(total, (8192 + 8192) * 1024 * 1024);
    }

    #[test]
    fn parse_nvidia_smi_single_gpu() {
        let (used, total) = parse_nvidia_smi("1536, 8192").unwrap();
        assert_eq!(used, 1536 * 1024 * 1024);
        assert_eq!(total, 8192 * 1024 * 1024);
    }

    #[test]
    fn parse_nvidia_smi_empty_is_none() {
        assert!(parse_nvidia_smi("").is_none());
    }

    #[test]
    fn parse_nvidia_smi_garbage_is_none() {
        assert!(parse_nvidia_smi("NVIDIA-SMI has failed").is_none());
        assert!(parse_nvidia_smi("abc,def\n").is_none());
    }
}
