//! D3 on-target demonstration (built only with `--cfg euk_d3_demo`).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Boots the real scheduler on bare metal: an idle thread and two workers, each
//! on its own kernel stack, switched by the real `euk_switch_context`, started by
//! the real trampoline, exiting through the real exit hook, and reclaimed by the
//! idle thread only after the switch away. Output goes to the serial port and is
//! asserted by `scripts/boot_test.py`.
//!
//! Scope: cooperative switching only. There is no hardware timer or interrupt:
//! the idle thread supplies a SOFTWARE tick to drive sleep expiry. Interrupts
//! stay disabled throughout (no IDT is loaded). Single CPU. No user mode, FPU,
//! TLS or address-space switching.
use crate::dispatch::{self, Kernel, KernelSlot};
use crate::kstack::{GUARD_BYTES, StackPool};
use crate::kthread::{Config, MemError, StackMemory, ThreadParams};
use crate::spin::SpinLock;
use crate::switch;
use crate::thread::{BlockReason, Handle, Priority};
use crate::{MAP_LIMIT, fail, finish, serial};

const SLOTS: usize = 6;
const STACK_BYTES: u64 = 16 * 1024;
const POOL_BYTES: usize = SLOTS * (STACK_BYTES + GUARD_BYTES) as usize;

#[repr(C, align(4096))]
struct PoolMem([u8; POOL_BYTES]);

/// Backing memory for the kernel stacks, handed to `StackPool` once at start.
static mut POOL: PoolMem = PoolMem([0; POOL_BYTES]);

/// Stack memory access over the identity-mapped window, restricted to `POOL`.
/// Volatile byte loops: no libc `memcpy`/`memset` dependency, and the scrub of a
/// freed stack cannot be optimised away.
struct RawMem {
    lo: usize,
    hi: usize,
}

impl RawMem {
    fn span(&self, addr: u64, len: usize) -> Result<usize, MemError> {
        let a = usize::try_from(addr).map_err(|_| MemError)?;
        let e = a.checked_add(len).ok_or(MemError)?;
        if a < self.lo || e > self.hi {
            return Err(MemError);
        }
        Ok(a)
    }
}

impl StackMemory for RawMem {
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), MemError> {
        let a = self.span(addr, data.len())?;
        for (i, b) in data.iter().enumerate() {
            // SAFETY: [a, a + len) is inside POOL (checked by `span`), which is
            // identity-mapped, writable and exclusively owned by the stack manager.
            unsafe { core::ptr::write_volatile((a + i) as *mut u8, *b) };
        }
        Ok(())
    }
    fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError> {
        let n = usize::try_from(len).map_err(|_| MemError)?;
        let a = self.span(addr, n)?;
        for i in 0..n {
            // SAFETY: as in `write`.
            unsafe { core::ptr::write_volatile((a + i) as *mut u8, 0) };
        }
        Ok(())
    }
}

static KERNEL: KernelSlot<SLOTS, RawMem> = KernelSlot::new();
static T1: SpinLock<Option<Handle>> = SpinLock::new(None);

unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
}

fn serial_u64(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if let Ok(s) = core::str::from_utf8(&buf[i..]) {
        serial(s);
    }
}

fn ok<T, E>(r: Result<T, E>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(_) => fail(what),
    }
}

extern "C" fn first_run() {
    ok(KERNEL.with(|k| k.complete_switch()), "D3 complete_switch");
}

extern "C" fn exit_hook(status: i32) -> ! {
    // SAFETY: single CPU, interrupts never enabled, KERNEL is a static, hooks
    // installed, no spinlock held here.
    unsafe {
        dispatch::exit_current_with(&KERNEL, status, |k| {
            // Terminated, but this code still runs on the thread's own stack.
            let me = match k.sched().current() {
                Some(h) => h,
                None => fail("D3 exit without current"),
            };
            match k.destroy(me) {
                Err(crate::kthread::Error::Thread(crate::thread::Error::StillOnCpu)) => {
                    serial("EUK D3 exit-path: self-destroy refused (still on cpu)\n")
                }
                _ => fail("D3 reclaimed a stack that is still in use"),
            }
        })
    }
}

extern "C" fn t1(_: u64) -> i32 {
    serial("EUK D3 T1 start\n");
    // SAFETY (all dispatch calls below): single CPU, IF clear, static kernel.
    ok(unsafe { dispatch::yield_now(&KERNEL) }, "D3 yield");
    serial("EUK D3 T1 resumed after yield\n");
    ok(
        unsafe { dispatch::block_current(&KERNEL, BlockReason::Event(1)) },
        "D3 block",
    );
    serial("EUK D3 T1 woken from block\n");
    serial("EUK D3 T1 sleeping 3 ticks\n");
    ok(unsafe { dispatch::sleep_current(&KERNEL, 3) }, "D3 sleep");
    serial("EUK D3 T1 awake after sleep\n");
    0
}

extern "C" fn t2(_: u64) -> i32 {
    serial("EUK D3 T2 start\n");
    ok(unsafe { dispatch::yield_now(&KERNEL) }, "D3 yield");
    serial("EUK D3 T2 resumed after yield\n");
    let target = match *T1.lock() {
        Some(h) => h,
        None => fail("D3 T1 handle missing"),
    };
    ok(dispatch::wake(&KERNEL, target), "D3 wake");
    serial("EUK D3 T2 woke T1\n");
    5
}

extern "C" fn idle(_: u64) -> i32 {
    let mut iterations = 0u32;
    loop {
        let live = KERNEL.with(|k| {
            k.reap_terminated(|id, status| {
                serial("EUK D3 idle reaped id=");
                serial_u64(id.0);
                serial(" status=");
                serial_u64(status as u64);
                serial("\n");
            });
            k.sched().live_threads()
        });
        if live == 1 {
            let (stacks, created, destroyed, switches, consistent) = KERNEL.with(|k| {
                (
                    k.stacks().stacks_in_use(),
                    k.stacks().created_total(),
                    k.stacks().destroyed_total(),
                    k.switch_count(),
                    k.check_accounting().is_ok() && k.sched().check_invariants().is_ok(),
                )
            });
            serial("EUK D3 stacks_in_use=");
            serial_u64(stacks as u64);
            serial(" live=1 created=");
            serial_u64(created);
            serial(" destroyed=");
            serial_u64(destroyed);
            serial("\nEUK D3 context_switches=");
            serial_u64(switches);
            serial("\n");
            if !consistent || stacks != 1 || created != destroyed + 1 {
                fail("D3 accounting");
            }
            serial("EUK D3 DEMO_OK\n");
            finish(true);
        }
        iterations += 1;
        if iterations > 100_000 {
            fail("D3 demo did not finish");
        }
        // Software tick standing in for the (not yet integrated) hardware timer.
        dispatch::tick(&KERNEL);
        ok(unsafe { dispatch::yield_now(&KERNEL) }, "D3 idle yield");
    }
}

fn entry(f: extern "C" fn(u64) -> i32) -> u64 {
    f as *const () as usize as u64
}

pub fn run() -> ! {
    switch::install_hooks(first_run, exit_hook);
    // Only the address is taken; POOL is used solely through RawMem.
    let lo = core::ptr::addr_of_mut!(POOL) as usize;
    let hi = lo + POOL_BYTES;
    let pool = ok(
        StackPool::new(lo as u64, POOL_BYTES as u64, STACK_BYTES, MAP_LIMIT as u64),
        "D3 stack pool",
    );
    // Linker-provided symbols; only their addresses are used.
    let text_start = core::ptr::addr_of!(__text_start) as usize as u64;
    let text_end = core::ptr::addr_of!(__text_end) as usize as u64;
    let cfg = Config {
        text_start,
        text_end,
        trampoline: switch::trampoline_address(),
    };
    KERNEL.install(ok(
        Kernel::new(cfg, pool, RawMem { lo, hi }),
        "D3 kernel init",
    ));
    let prio = match Priority::new(4) {
        Ok(p) => p,
        Err(_) => fail("D3 priority"),
    };
    KERNEL.with(|k| {
        ok(k.create_idle(entry(idle), 0), "D3 create idle");
        let a = ok(
            k.create(ThreadParams {
                entry: entry(t1),
                arg: 0,
                priority: prio,
            }),
            "D3 create T1",
        );
        *T1.lock() = Some(a);
        ok(
            k.create(ThreadParams {
                entry: entry(t2),
                arg: 0,
                priority: prio,
            }),
            "D3 create T2",
        );
    });
    serial("EUK D3: idle and two workers created\n");
    // The first dispatch abandons this bootstrap context; it is never resumed.
    // SAFETY: single CPU, IF clear, static kernel, hooks installed.
    let r = unsafe { dispatch::reschedule(&KERNEL) };
    let _ = r;
    fail("D3 bootstrap context resumed")
}
