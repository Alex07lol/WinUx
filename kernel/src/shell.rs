//! Minimal serial shell (Phase 6).
//!
//! Runs on its own scheduler thread after the boot self-tests report success.
//! It reads lines from the serial console (IRQ4-driven) and executes a small
//! set of builtins over the kernel services: frame allocator, kernel heap,
//! scheduler, timer and the in-memory VFS. There is no line history or job
//! control; output is serial-only.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::{pit, read_tsc};
use crate::sync::IrqSpinLock;
use crate::{blk, console, heap, memory, persist, sched, serial, vfs};

static BUILD_ID: IrqSpinLock<&'static str> = IrqSpinLock::new("dev");
static TSC_FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// Store the values captured during boot; call before spawning the shell.
pub fn configure(build_id: &'static str, tsc_frequency: Option<u64>) {
    *BUILD_ID.lock() = build_id;
    TSC_FREQUENCY.store(tsc_frequency.unwrap_or(0), Ordering::SeqCst);
}

/// Shell thread entry: never returns.
pub fn run() -> ! {
    serial::force_write_str("\r\nPROJECT_OS shell. Type 'help' for commands.\r\n");
    let mut line = String::new();
    loop {
        serial::force_write_str("project-os> ");
        console::read_line(&mut line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (command, args) = match trimmed.find(' ') {
            Some(index) => (&trimmed[..index], trimmed[index + 1..].trim()),
            None => (trimmed, ""),
        };
        if let Err(message) = execute(command, args) {
            crate::kprintln!("error: {}", message);
        }
        persist::flush_if_dirty();
    }
}

/// Split `"<path> <text>"` into its two parts; text may contain spaces and is
/// taken verbatim after the first separating space.
fn split_path_and_text(args: &str) -> Option<(&str, &str)> {
    if args.is_empty() {
        return None;
    }
    match args.find(' ') {
        Some(index) => Some((&args[..index], &args[index + 1..])),
        None => Some((args, "")),
    }
}

fn execute(command: &str, args: &str) -> Result<(), &'static str> {
    match command {
        "help" => {
            crate::kprintln!("commands:");
            crate::kprintln!("  help              this list");
            crate::kprintln!("  version           kernel and build information");
            crate::kprintln!("  echo <text>       print text back");
            crate::kprintln!("  mem               frame allocator statistics");
            crate::kprintln!("  heap              kernel heap statistics and validation");
            crate::kprintln!("  ticks             PIT tick count");
            crate::kprintln!("  uptime            time since boot (ticks and TSC)");
            crate::kprintln!("  threads           scheduler statistics");
            crate::kprintln!("  ls [path]         list a directory (default /)");
            crate::kprintln!("  cat <path>        print a file");
            crate::kprintln!("  write <path> <text...>   create or replace a file");
            crate::kprintln!("  append <path> <text...>  append to a file");
            crate::kprintln!("  mkdir <path>      create a directory");
            crate::kprintln!("  rm <path>         remove a file or empty directory");
            crate::kprintln!("  stat <path>       node kind and size");
            crate::kprintln!("  vfs               re-run the VFS self-test");
            crate::kprintln!("  blk               ATA device information");
            crate::kprintln!("  persist           checkpoint image status");
            crate::kprintln!("  console           serial receive statistics");
            crate::kprintln!("  clear             clear the terminal");
        }
        "version" => {
            crate::kprintln!(
                "PROJECT_OS {} build {} (x86_64, Limine base revision {})",
                os_core::VERSION,
                *BUILD_ID.lock(),
                os_core::LIMINE_BASE_REVISION
            );
            crate::kprintln!(
                "user-mode syscall gate: int 0x80; VFS: checkpoint image on ATA disk when present"
            );
        }
        "echo" => crate::kprintln!("{}", args),
        "mem" => {
            let stats = memory::stats()?;
            crate::kprintln!(
                "frames: {} free / {} total ({} MiB managed), largest run {} frames",
                stats.free_frames,
                stats.total_frames,
                stats.total_bytes / (1024 * 1024),
                stats.largest_free_run
            );
        }
        "heap" => {
            let stats = heap::stats().map_err(|_| "heap stats failed")?;
            crate::kprintln!(
                "heap: {} KiB free / {} KiB total, {} blocks, largest {} B",
                stats.free_bytes / 1024,
                stats.region_bytes / 1024,
                stats.block_count,
                stats.largest_free_block
            );
            let validated = heap::validate().map_err(|_| "heap validation FAILED")?;
            crate::kprintln!(
                "heap validate: ok ({} free bytes, {} blocks)",
                validated.free_bytes,
                validated.block_count
            );
        }
        "ticks" => crate::kprintln!("ticks: {}", pit::ticks()),
        "uptime" => {
            let ticks = pit::ticks();
            let hz = pit::effective_hz() as u64;
            let tsc_frequency = TSC_FREQUENCY.load(Ordering::SeqCst);
            crate::kprint!(
                "uptime: {}.{:03} s ({} ticks at {} Hz)",
                ticks / hz,
                (ticks % hz) * 1000 / hz,
                ticks,
                hz
            );
            if let Some(seconds) = read_tsc().checked_div(tsc_frequency) {
                let millis = (read_tsc() % tsc_frequency) * 1000 / tsc_frequency;
                crate::kprint!(", TSC {}.{:03} s", seconds, millis);
            }
            crate::kprintln!();
        }
        "threads" => {
            let stats = sched::stats();
            crate::kprintln!(
                "threads: {} live, {} switches, {} preemptions",
                stats.live,
                stats.switches,
                stats.preemptions
            );
        }
        "ls" => {
            let path = if args.is_empty() { "/" } else { args };
            let entries = vfs::with(|vfs| vfs.list(path))?.map_err(|error| error.label())?;
            crate::kprintln!("{} entries in {}:", entries.len(), path);
            for (name, kind, size) in entries {
                match kind {
                    vfs::NodeKind::Dir => crate::kprintln!("  {}/", name),
                    vfs::NodeKind::File => crate::kprintln!("  {} ({} B)", name, size),
                }
            }
        }
        "cat" => {
            if args.is_empty() {
                return Err("usage: cat <path>");
            }
            let data = vfs::with(|vfs| vfs.read_file(args).map(|data| data.to_vec()))?
                .map_err(|error| error.label())?;
            serial::force_write_bytes(&data);
            if !data.ends_with(b"\n") {
                serial::force_write_str("\r\n");
            }
        }
        "write" | "append" => {
            let Some((path, text)) = split_path_and_text(args) else {
                return Err("usage: write/append <path> <text>");
            };
            if command == "write" {
                vfs::with(|vfs| vfs.write_file(path, text.as_bytes()))?
                    .map_err(|error| error.label())?;
            } else {
                vfs::with(|vfs| vfs.append_file(path, text.as_bytes()))?
                    .map_err(|error| error.label())?;
            }
            crate::kprintln!("{}: ok", command);
        }
        "mkdir" => {
            if args.is_empty() {
                return Err("usage: mkdir <path>");
            }
            vfs::with(|vfs| vfs.mkdir(args))?.map_err(|error| error.label())?;
            crate::kprintln!("mkdir: ok");
        }
        "rm" => {
            if args.is_empty() {
                return Err("usage: rm <path>");
            }
            vfs::with(|vfs| vfs.remove(args))?.map_err(|error| error.label())?;
            crate::kprintln!("rm: ok");
        }
        "stat" => {
            if args.is_empty() {
                return Err("usage: stat <path>");
            }
            let (kind, size) = vfs::with(|vfs| vfs.stat(args))?.map_err(|error| error.label())?;
            match kind {
                vfs::NodeKind::File => crate::kprintln!("{}: file ({} B)", args, size),
                vfs::NodeKind::Dir => crate::kprintln!("{}: directory ({} entries)", args, size),
            }
        }
        "vfs" => {
            let (nodes, files) = vfs::self_test()?;
            crate::kprintln!(
                "vfs selftest: passed ({} nodes, {} files, error paths checked)",
                nodes,
                files
            );
        }
        "blk" => match blk::info() {
            Some(info) => {
                let stats = blk::stats();
                crate::kprintln!(
                    "blk: {}, {} sectors ({} MiB), lba28 pio; reads {} sectors, writes {} sectors, flushes {}, errors {}",
                    info.model,
                    info.sectors,
                    info.sectors / 2048,
                    stats.sectors_read,
                    stats.sectors_written,
                    stats.flushes,
                    stats.errors
                );
            }
            None => crate::kprintln!("blk: no ATA device on primary channel"),
        },
        "persist" => {
            let status = persist::status();
            match status.active {
                Some((slot, seq)) => {
                    let last_load = match status.last_load {
                        Some((slot, seq)) => alloc::format!("slot {} seq {}", slot.name(), seq),
                        None => String::from("none"),
                    };
                    crate::kprintln!(
                        "persist: active slot {} seq {}; saves {}, loads {}, last load {}, errors {}",
                        slot.name(),
                        seq,
                        status.saves,
                        status.loads,
                        last_load,
                        status.errors
                    );
                }
                None => crate::kprintln!("persist: inactive (no ATA disk)"),
            }
        }
        "console" => {
            crate::kprintln!(
                "console: rx ring {}/{} bytes pending, {} overruns (uart fifo or ring)",
                console::ring_len(),
                console::RX_CAPACITY,
                console::overruns()
            );
        }
        "clear" => {
            serial::force_write_str("\x1b[2J\x1b[H");
            crate::fbcon::clear();
        }
        _ => crate::kprintln!("unknown command: {} (try 'help')", command),
    }
    Ok(())
}
