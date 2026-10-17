//! Host-side benchmark for the physical-frame bitmap allocator.
//!
//! This measures the *host model* of the allocator (compiled for the build
//! machine, using a heap bitmap), not the kernel build. Results are only
//! comparable across runs with identical arguments, compiler and machine, and
//! they are not a comparison against Linux.

use std::process::ExitCode;
use std::time::Instant;

use os_core::frame::{FrameAllocator, PAGE_SIZE};
use os_core::rng::XorShift64;

const MIB: u64 = 1024 * 1024;

struct Config {
    mib: u64,
    iterations: usize,
    seed: u64,
    max_live: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mib: 64,
            iterations: 200_000,
            seed: 0x5052_4f4a_4543_5401,
            max_live: 4096,
        }
    }
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing value for {arg}"))
        };
        match arg.as_str() {
            "--mib" => config.mib = value()?.parse().map_err(|_| "invalid --mib")?,
            "--iterations" => {
                config.iterations = value()?.parse().map_err(|_| "invalid --iterations")?
            }
            "--seed" => config.seed = value()?.parse().map_err(|_| "invalid --seed")?,
            "--max-live" => config.max_live = value()?.parse().map_err(|_| "invalid --max-live")?,
            "--help" | "-h" => {
                println!("usage: membench [--mib N] [--iterations N] [--seed N] [--max-live N]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if config.mib == 0 || config.iterations == 0 || config.max_live == 0 {
        return Err("all numeric arguments must be greater than zero".into());
    }
    Ok(config)
}

fn percentile(sorted: &[u128], fraction: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn main() -> ExitCode {
    let config = match parse_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("membench: {error}");
            return ExitCode::FAILURE;
        }
    };

    let max_address = config.mib * MIB;
    let frames = (max_address / PAGE_SIZE) as usize;
    let mut bits = vec![0u64; frames.div_ceil(64)];
    let mut allocator = FrameAllocator::new(&mut bits, max_address);

    // Use 3/4 of the region so the allocator is not forced to fail constantly.
    let usable = (config.mib / 4 * 3) * MIB;
    allocator.add_usable_region(0, usable).unwrap();
    let baseline_free = allocator.stats().free_frames;

    println!("PROJECT_OS frame allocator host benchmark");
    println!(
        "config: mib={} usable_mib={} iterations={} seed=0x{:016x} max_live={}",
        config.mib,
        usable / MIB,
        config.iterations,
        config.seed,
        config.max_live
    );
    println!(
        "build: profile={} arch={} os={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!(
        "frames: total={} usable={} largest_run={}",
        allocator.stats().total_frames,
        baseline_free,
        allocator.stats().largest_free_run
    );

    let mut rng = XorShift64::new(config.seed);
    let mut live = Vec::with_capacity(config.max_live);
    let mut batches: Vec<u128> = Vec::with_capacity(config.iterations / 64 + 1);
    let mut batch_start = Instant::now();
    let mut ops_in_batch = 0u64;
    let mut allocations = 0u64;
    let mut frees = 0u64;
    let overall_start = Instant::now();

    for iteration in 0..config.iterations {
        let allocate = live.is_empty()
            || (live.len() < config.max_live && rng.next_bool())
            || live.len() >= config.max_live;

        if allocate {
            // A mixed order distribution keeps the scan path exercised.
            let order = 1usize << (rng.next_u64() % 3);
            match allocator.alloc_pages(order) {
                Some(frame) => {
                    live.push((frame, order));
                    allocations += 1;
                }
                None => {
                    // Exhausted: release something and keep the workload going.
                    if !live.is_empty() {
                        let index = rng.next_usize(live.len());
                        let (frame, order) = live.swap_remove(index);
                        allocator.free_pages(frame, order).unwrap();
                        frees += 1;
                    }
                }
            }
        } else {
            let index = rng.next_usize(live.len());
            let (frame, order) = live.swap_remove(index);
            allocator.free_pages(frame, order).unwrap();
            frees += 1;
        }

        ops_in_batch += 1;
        if ops_in_batch == 64 || iteration + 1 == config.iterations {
            let nanos = batch_start.elapsed().as_nanos();
            batches.push(nanos / ops_in_batch as u128);
            batch_start = Instant::now();
            ops_in_batch = 0;
        }
    }

    for (frame, order) in live {
        allocator.free_pages(frame, order).unwrap();
        frees += 1;
    }
    let elapsed = overall_start.elapsed();
    let total_ops = allocations + frees;
    let final_stats = allocator.stats();
    if final_stats.free_frames != baseline_free {
        eprintln!(
            "membench: allocator accounting mismatch: baseline={} final={}",
            baseline_free, final_stats.free_frames
        );
        return ExitCode::FAILURE;
    }

    batches.sort_unstable();
    let mean_ns = elapsed.as_nanos() as f64 / total_ops.max(1) as f64;
    println!(
        "result: ops={} allocations={} frees={} elapsed_ms={:.3} mean_ns_per_op={:.1}",
        total_ops,
        allocations,
        frees,
        elapsed.as_secs_f64() * 1000.0,
        mean_ns
    );
    println!(
        "distribution: batches={} p50_batch_ns_per_op={} p99_batch_ns_per_op={}",
        batches.len(),
        percentile(&batches, 0.50),
        percentile(&batches, 0.99)
    );
    println!(
        "post_churn: free_frames={} largest_free_run={} fragmentation_free_ratio={:.4}",
        final_stats.free_frames,
        final_stats.largest_free_run,
        final_stats.largest_free_run as f64 / final_stats.free_frames.max(1) as f64
    );
    println!("accounting: OK (all frames returned)");
    ExitCode::SUCCESS
}
