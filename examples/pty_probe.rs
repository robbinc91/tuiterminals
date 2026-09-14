//! Diagnostic: open a PTY exactly like the app does, spawn the default shell,
//! and probe the ConPTY I/O round-trip. Bypasses ratatui/crossterm so we can
//! tell whether the PTY/shell layer is the problem. Run with:
//!   cargo run --example pty_probe
//!
//! Phases:
//!   1  read startup output (expect a lone `\x1b[6n` CPR request)
//!   2  RESPOND to the CPR with `\x1b[1;1R` and see if output unblocks
//!   3  resize to a DIFFERENT size and see if output unblocks
//!   4  send a command and see if it echoes back
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drain up to `limit` of pending channel messages, printing each chunk.
/// Returns total bytes read.
fn drain(rx: &mpsc::Receiver<Vec<u8>>, label: &str, limit: Duration) -> usize {
    let start = Instant::now();
    let mut total = 0usize;
    while start.elapsed() < limit {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(bytes) => {
                total += bytes.len();
                eprintln!(
                    "\n--- {label} chunk ({} bytes) hex: {} ---\n{}",
                    bytes.len(),
                    hex(&bytes),
                    String::from_utf8_lossy(&bytes)
                );
            }
            Err(_) => {
                if start.elapsed() >= limit {
                    break;
                }
            }
        }
    }
    total
}

fn main() {
    let ps = native_pty_system();
    let size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = ps.openpty(size).expect("openpty failed");
    let slave = pair.slave;
    let master = pair.master;

    eprintln!("opened pty at {rows}x{cols}", rows = size.rows, cols = size.cols);
    eprintln!("spawning default shell...");
    let mut child = slave
        .spawn_command(CommandBuilder::new_default_prog())
        .expect("spawn_command failed");
    eprintln!("spawn_command returned ok");

    let reader = master.try_clone_reader().expect("try_clone_reader failed");
    let mut writer = master.take_writer().expect("take_writer failed");

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // ---- Phase 1: initial startup output ----
    eprintln!("\n== phase 1: initial output (up to 3s) ==");
    let p1 = drain(&rx, "p1", Duration::from_secs(3));
    eprintln!("\nphase 1 total bytes: {p1}");
    let saw_cpr = p1 >= 4; // at least the 4-byte CPR request

    // ---- Phase 2: respond to the CPR request ----
    if saw_cpr {
        eprintln!("\n== phase 2: responding to CPR with '\\x1b[1;1R' (up to 3s) ==");
        match writer.write_all(b"\x1b[1;1R") {
            Ok(()) => eprintln!("cpr response written"),
            Err(e) => eprintln!("cpr response write FAILED: {e}"),
        }
        let p2 = drain(&rx, "p2", Duration::from_secs(3));
        eprintln!("\nphase 2 total bytes: {p2}");
    } else {
        eprintln!("\n== phase 2: skipped (no CPR seen) ==");
    }

    // ---- Phase 3: resize to a DIFFERENT size ----
    eprintln!("\n== phase 3: resize to 25x81 (up to 3s) ==");
    let new_size = PtySize {
        rows: 25,
        cols: 81,
        pixel_width: 0,
        pixel_height: 0,
    };
    let _ = master.resize(new_size);
    let p3 = drain(&rx, "p3", Duration::from_secs(3));
    eprintln!("\nphase 3 total bytes: {p3}");

    // ---- Phase 4: send a command ----
    eprintln!("\n== phase 4: sending 'echo PING123' + Enter (up to 3s) ==");
    match writer.write_all(b"echo PING123\r") {
        Ok(()) => eprintln!("write ok"),
        Err(e) => eprintln!("write FAILED: {e}"),
    }
    let p4 = drain(&rx, "p4", Duration::from_secs(3));
    eprintln!("\nphase 4 total bytes: {p4}");

    match child.try_wait() {
        Ok(Some(status)) => eprintln!("\nfinal: child EXITED: {status}"),
        Ok(None) => eprintln!("\nfinal: child still ALIVE"),
        Err(e) => eprintln!("\nfinal: try_wait error: {e}"),
    }

    // Bounded final wait so we never hang the terminal.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if let Ok(Some(status)) = child.try_wait() {
            eprintln!("\nfinal: child exited: {status}");
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("\nfinal: child still alive after 2s (leaving it; reaped on exit)");
}
