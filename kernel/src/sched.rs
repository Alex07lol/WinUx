//! Kernel threads and a round-robin scheduler (Phase 5a).
//!
//! Single-CPU design:
//!
//! * The boot context becomes thread 0 (`boot`); its context is captured the
//!   first time it is switched away from.
//! * A fixed table of [`MAX_THREADS`] threads, each with its own 16 KiB kernel
//!   stack allocated from the frame allocator.
//! * Switching is cooperative (`yield_now`, `exit`) or preemptive: the timer
//!   ISR sets a reschedule request when another thread is runnable, and the
//!   switch happens at the tail of the interrupt dispatcher, so the interrupted
//!   thread's interrupt frame stays on its own stack and resumes through the
//!   normal `iretq` path.
//! * Interrupts are disabled from the moment the current thread's stack
//!   pointer can be captured until the switch completes, and stay disabled on
//!   the new thread's entry until its trampoline re-enables them. That is what
//!   makes "capture rsp, then switch" atomic against the timer on one CPU.
//!
//! Known limits (documented, not hidden): thread stacks are not freed when a
//! thread exits, a finished slot is not reused, and there is no priority,
//! sleeping, or SMP support yet.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use os_core::frame::PAGE_SIZE;

use crate::arch::x86_64::context::project_os_context_switch;
use crate::arch::x86_64::{
    halt, interrupts_disable, interrupts_enable, interrupts_restore, interrupts_save_disable,
    read_tsc,
};
use crate::kprintln;
use crate::memory;
use crate::sync::IrqSpinLock;

pub const MAX_THREADS: usize = 16;
const STACK_PAGES: usize = 4;

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ThreadState {
    Unused,
    Ready,
    Running,
    Finished,
}

#[derive(Clone, Copy)]
struct Thread {
    rsp: u64,
    stack_physical: u64,
    entry: Option<fn()>,
    name: &'static str,
    id: u64,
    state: ThreadState,
}

impl Thread {
    const fn empty() -> Self {
        Self {
            rsp: 0,
            stack_physical: 0,
            entry: None,
            name: "",
            id: 0,
            state: ThreadState::Unused,
        }
    }
}

struct ThreadTable(UnsafeCell<[Thread; MAX_THREADS]>);

// SAFETY: all reads and writes of the table happen under the `SCHED` lock,
// except the `rsp` slot of the thread being switched away from. That thread is
// not runnable at that moment (the scheduler already moved on), so no other
// context can reach it.
unsafe impl Sync for ThreadTable {}

static THREADS: ThreadTable =
    ThreadTable(UnsafeCell::new([const { Thread::empty() }; MAX_THREADS]));

fn threads() -> &'static mut [Thread; MAX_THREADS] {
    // SAFETY: callers hold the `SCHED` lock for the duration of their access.
    unsafe { &mut *THREADS.0.get() }
}

#[derive(Clone, Copy)]
pub struct SchedStats {
    pub live: usize,
    pub switches: u64,
    pub preemptions: u64,
}

struct Scheduler {
    current: Option<usize>,
    live: usize,
    next_id: u64,
    preempt_enabled: bool,
    need_resched: bool,
    switches: u64,
    preemptions: u64,
    /// Physical base of a finished thread's stack, freed at the next
    /// scheduling point (never from the dead thread itself).
    reclaim_pending: Option<u64>,
}

static SCHED: IrqSpinLock<Scheduler> = IrqSpinLock::new(Scheduler {
    current: None,
    live: 0,
    next_id: 0,
    preempt_enabled: false,
    need_resched: false,
    switches: 0,
    preemptions: 0,
    reclaim_pending: None,
});

/// Free a departed thread's stack. Must only be called at a scheduling point
/// reached by a *different* thread than the one that exited.
fn reclaim_finished_stack() {
    let pending = SCHED.lock().reclaim_pending.take();
    let Some(physical) = pending else {
        return;
    };
    if let Some(frame) = os_core::frame::PhysFrame::from_address(physical) {
        let _ = memory::free_pages(frame, STACK_PAGES);
    }
}

/// Initialise the scheduler and adopt the calling context as thread 0.
pub fn init(hhdm_offset: u64) -> Result<(), &'static str> {
    let mut sched = SCHED.lock();
    if sched.current.is_some() {
        return Err("scheduler already initialised");
    }
    let threads = threads();
    threads[0] = Thread {
        rsp: 0,
        stack_physical: 0,
        entry: None,
        name: "boot",
        id: 0,
        state: ThreadState::Running,
    };
    sched.current = Some(0);
    sched.live = 1;
    sched.next_id = 1;
    HHDM_OFFSET.store(hhdm_offset, Ordering::SeqCst);
    Ok(())
}

/// Round-robin scan starting after `current`, returning the next runnable
/// thread. Only `Ready` threads are eligible; `Running` is the caller.
fn pick_next(current: usize, threads: &[Thread; MAX_THREADS]) -> Option<usize> {
    for offset in 1..=MAX_THREADS {
        let index = (current + offset) % MAX_THREADS;
        if threads[index].state == ThreadState::Ready {
            return Some(index);
        }
    }
    None
}

/// Create a new kernel thread that starts running `entry` on first schedule.
pub fn spawn(name: &'static str, entry: fn()) -> Result<u64, &'static str> {
    let hhdm = HHDM_OFFSET.load(Ordering::SeqCst);
    if hhdm == 0 {
        return Err("scheduler is not initialised");
    }
    let mut sched = SCHED.lock();
    if sched.current.is_none() {
        return Err("scheduler is not initialised");
    }
    let threads = threads();
    let slot = threads
        .iter()
        .position(|thread| thread.state == ThreadState::Unused)
        .ok_or("thread table is full")?;

    let stack = memory::alloc_pages(STACK_PAGES)?;
    let stack_physical = stack.address();
    let stack_bytes = (STACK_PAGES * PAGE_SIZE as usize) as u64;
    let base = hhdm
        .checked_add(stack_physical)
        .ok_or("thread stack HHDM overflow")?;

    // Fabricated switch-in frame, low to high: r15, r14, r13, r12, rbx, rbp,
    // return address (the trampoline). After the six pops and the return, the
    // trampoline starts with rsp = base + bytes - 8, which satisfies the SysV
    // rule that a function entered by `call`/`ret` sees rsp % 16 == 8.
    let entry_rsp = base + stack_bytes - 8;
    let saved_registers = entry_rsp - 56;
    // SAFETY: the stack frames were just allocated for this thread and nothing
    // else writes them before the first switch-in.
    unsafe {
        let slot_ptr = saved_registers as *mut u64;
        for index in 0..6 {
            slot_ptr.add(index).write(0);
        }
        slot_ptr
            .add(6)
            .write(thread_trampoline as *const () as usize as u64);
    }

    let id = sched.next_id;
    sched.next_id += 1;
    threads[slot] = Thread {
        rsp: saved_registers,
        stack_physical,
        entry: Some(entry),
        name,
        id,
        state: ThreadState::Ready,
    };
    sched.live += 1;
    Ok(id)
}

/// Entry point for freshly spawned threads. The scheduler has already made
/// this thread current; interrupts were left disabled by the switch.
extern "C" fn thread_trampoline() -> ! {
    let entry = {
        let sched = SCHED.lock();
        let current = sched.current.unwrap_or(0);
        threads()[current].entry.take()
    };
    interrupts_enable();
    if let Some(entry) = entry {
        entry();
    }
    exit()
}

/// Yield the CPU to the next runnable thread; returns when the caller is
/// scheduled again (or immediately when no other thread is runnable).
pub fn yield_now() {
    reclaim_finished_stack();
    let flags = interrupts_save_disable();
    let switch = {
        let mut sched = SCHED.lock();
        let current = sched.current.unwrap_or(0);
        let threads = threads();
        if threads[current].state == ThreadState::Running {
            threads[current].state = ThreadState::Ready;
        }
        match pick_next(current, threads) {
            Some(next) => {
                threads[next].state = ThreadState::Running;
                sched.current = Some(next);
                sched.switches += 1;
                Some((
                    core::ptr::addr_of_mut!(threads[current].rsp),
                    threads[next].rsp,
                ))
            }
            None => {
                threads[current].state = ThreadState::Running;
                None
            }
        }
    };
    if let Some((old_rsp, new_rsp)) = switch {
        // SAFETY: `old_rsp` points into the current thread's table slot and
        // `new_rsp` is a stack prepared by `spawn` or saved by a previous
        // switch. Interrupts are disabled across the switch.
        unsafe { project_os_context_switch(old_rsp, new_rsp) };
    }
    interrupts_restore(flags);
}

/// Terminate the calling thread and never return.
pub fn exit() -> ! {
    // Disabled for the rest of this context's life; the thread is finished and
    // never resumed, so the saved flags are intentionally discarded.
    let _flags = interrupts_save_disable();
    let switch = {
        let mut sched = SCHED.lock();
        let current = sched.current.unwrap_or(0);
        let threads = threads();
        threads[current].state = ThreadState::Finished;
        sched.live = sched.live.saturating_sub(1);
        // Deferred: the exiting thread is still running on this stack, so a
        // later scheduling point on another thread frees it.
        sched.reclaim_pending = Some(threads[current].stack_physical);
        match pick_next(current, threads) {
            Some(next) => {
                threads[next].state = ThreadState::Running;
                sched.current = Some(next);
                sched.switches += 1;
                Some((
                    core::ptr::addr_of_mut!(threads[current].rsp),
                    threads[next].rsp,
                ))
            }
            None => None,
        }
    };
    if let Some((old_rsp, new_rsp)) = switch {
        // SAFETY: as in `yield_now`; the dead thread's stack is never resumed
        // because `Finished` threads are not schedulable.
        unsafe { project_os_context_switch(old_rsp, new_rsp) };
    }
    interrupts_disable();
    crate::serial::force_write_str("\r\nPROJECT_OS_FATAL: no runnable thread on exit\r\n");
    loop {
        halt();
    }
}

/// Timer-ISR hook: request a reschedule when another thread is runnable.
pub fn on_timer_tick() {
    let mut sched = SCHED.lock();
    if !sched.preempt_enabled {
        return;
    }
    let current = sched.current.unwrap_or(0);
    let threads = threads();
    if threads
        .iter()
        .enumerate()
        .any(|(index, thread)| index != current && thread.state == ThreadState::Ready)
    {
        sched.need_resched = true;
    }
}

/// Interrupt-dispatcher tail: perform a pending reschedule, if any.
pub fn preempt_if_needed() {
    reclaim_finished_stack();
    let flags = interrupts_save_disable();
    let switch = {
        let mut sched = SCHED.lock();
        if !sched.need_resched {
            None
        } else {
            sched.need_resched = false;
            let current = sched.current.unwrap_or(0);
            let threads = threads();
            if threads[current].state == ThreadState::Running {
                threads[current].state = ThreadState::Ready;
            }
            match pick_next(current, threads) {
                Some(next) => {
                    threads[next].state = ThreadState::Running;
                    sched.current = Some(next);
                    sched.switches += 1;
                    sched.preemptions += 1;
                    Some((
                        core::ptr::addr_of_mut!(threads[current].rsp),
                        threads[next].rsp,
                    ))
                }
                None => {
                    threads[current].state = ThreadState::Running;
                    None
                }
            }
        }
    };
    if let Some((old_rsp, new_rsp)) = switch {
        // SAFETY: as in `yield_now`. The interrupted context resumes inside
        // its own ISR and unwinds through `iretq` with the saved frame.
        unsafe { project_os_context_switch(old_rsp, new_rsp) };
    }
    interrupts_restore(flags);
}

pub fn set_preemption(enabled: bool) {
    SCHED.lock().preempt_enabled = enabled;
}

pub fn live_count() -> usize {
    SCHED.lock().live
}

/// Kernel-stack top of the calling thread, used to program the TSS `rsp0`
/// before entering ring 3. `None` for the boot thread, whose stack is not
/// scheduler-owned (it never enters user mode).
pub fn current_kernel_stack_top() -> Option<u64> {
    let hhdm = HHDM_OFFSET.load(Ordering::SeqCst);
    let sched = SCHED.lock();
    let current = sched.current.unwrap_or(0);
    let thread = threads()[current];
    if thread.stack_physical == 0 || hhdm == 0 {
        return None;
    }
    let bytes = (STACK_PAGES * PAGE_SIZE as usize) as u64;
    hhdm.checked_add(thread.stack_physical)
        .map(|base| base + bytes)
}

pub fn stats() -> SchedStats {
    let sched = SCHED.lock();
    SchedStats {
        live: sched.live,
        switches: sched.switches,
        preemptions: sched.preemptions,
    }
}

/// Block (with `hlt`) until only thread 0 remains. Bounded by PIT ticks and,
/// when the bootloader provided a TSC frequency, by wall time as well.
pub fn wait_all_finished(
    timeout_ticks: u64,
    tsc_frequency: Option<u64>,
) -> Result<(), &'static str> {
    let start_ticks = crate::arch::x86_64::pit::ticks();
    let start_tsc = read_tsc();
    let tsc_limit = tsc_frequency.map(|frequency| frequency.saturating_mul(2));
    while live_count() > 1 {
        if crate::arch::x86_64::pit::ticks().saturating_sub(start_ticks) > timeout_ticks {
            return Err("threads did not finish within the tick budget");
        }
        if let Some(limit) = tsc_limit
            && read_tsc().wrapping_sub(start_tsc) > limit
        {
            return Err("threads did not finish within the TSC budget");
        }
        // With interrupts enabled, `hlt` wakes on the next timer tick, whose
        // ISR switches to runnable worker threads and back.
        halt();
    }
    Ok(())
}

/// Boot-time scheduler demonstration and self-test.
#[derive(Clone, Copy)]
pub struct DemoReport {
    pub worker_a_steps: u64,
    pub worker_b_steps: u64,
    pub spinner_done: bool,
    pub stats: SchedStats,
}

static DEMO_A: AtomicU64 = AtomicU64::new(0);
static DEMO_B: AtomicU64 = AtomicU64::new(0);
static DEMO_SPINNER: AtomicU64 = AtomicU64::new(0);

fn worker_a() {
    for step in 0..5 {
        DEMO_A.fetch_add(1, Ordering::SeqCst);
        kprintln!("sched: worker-a step {} (yield)", step);
        yield_now();
    }
}

fn worker_b() {
    for step in 0..5 {
        DEMO_B.fetch_add(1, Ordering::SeqCst);
        kprintln!("sched: worker-b step {} (yield)", step);
        yield_now();
    }
}

fn spinner() {
    // Long dependency chain that the optimizer cannot fold away; long enough
    // to be preempted several times at 100 Hz.
    let mut value = 0x1234_5678_9abc_def0u64;
    for _ in 0..40_000_000u64 {
        value = core::hint::black_box(
            value
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407),
        );
    }
    DEMO_SPINNER.store(value | 1, Ordering::SeqCst);
}

/// Run the boot demonstration: two yielding workers, one CPU-bound spinner,
/// timer preemption enabled. `init` must have been called first.
pub fn demo(tsc_frequency: Option<u64>) -> Result<DemoReport, &'static str> {
    spawn("worker-a", worker_a)?;
    spawn("worker-b", worker_b)?;
    spawn("spinner", spinner)?;
    set_preemption(true);
    wait_all_finished(300, tsc_frequency)?;
    set_preemption(false);

    let worker_a_steps = DEMO_A.load(Ordering::SeqCst);
    let worker_b_steps = DEMO_B.load(Ordering::SeqCst);
    let spinner_done = DEMO_SPINNER.load(Ordering::SeqCst) != 0;
    let stats = stats();
    if worker_a_steps != 5 || worker_b_steps != 5 {
        return Err("worker threads did not complete their steps");
    }
    if !spinner_done {
        return Err("spinner thread did not complete");
    }
    if stats.preemptions == 0 {
        return Err("no timer-driven preemptions were observed");
    }
    if stats.switches < 8 {
        return Err("too few context switches for the workload");
    }

    // Report the finished threads by name and id (also exercises those fields).
    let mut finished: [(&'static str, u64); 4] = [("", 0); 4];
    let mut finished_count = 0;
    {
        let _sched = SCHED.lock();
        for thread in threads()
            .iter()
            .filter(|thread| thread.state == ThreadState::Finished)
        {
            if finished_count < finished.len() {
                finished[finished_count] = (thread.name, thread.id);
                finished_count += 1;
            }
        }
    }
    for &(name, id) in &finished[..finished_count] {
        kprintln!("sched: thread {} ({}) finished", id, name);
    }

    Ok(DemoReport {
        worker_a_steps,
        worker_b_steps,
        spinner_done,
        stats,
    })
}
