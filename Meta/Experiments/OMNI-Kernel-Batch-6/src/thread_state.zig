//! Thread states and valid transitions (Batch 6).
//!
//! SPECIFICATION WITH A CHECKER. The state names are those of the existing
//! `Kernel/Tasks/Thread.h` (`Thread::State`): invalid, runnable, running, dying, dead,
//! stopped, blocked. The transition table below is this package's reading of
//! `Thread::set_state`, `Thread::block_impl` and `Thread::unblock` in
//! `Kernel/Tasks/Thread.cpp`:
//!   * setting the state a thread already has is a PANIC there, so it is an error here;
//!   * blocking requires the thread to be `running` ("Attempting to block with invalid
//!     thread state" is a PANIC);
//!   * `stopped` may only be entered from `running`;
//!   * `dying` must not be entered from `blocked` (`VERIFY(previous_state != Blocked)`):
//!     a blocked thread is woken first (interrupted by death) and then dies.
//! Where the C++ source is silent (for example `runnable -> dying`), the table follows the
//! way the source is used and says so in docs/SCHEDULER-INTEGRATION.md. Nothing in the
//! existing kernel calls this code.

const std = @import("std");

pub const State = enum(u8) {
    invalid,
    runnable,
    running,
    blocked,
    stopped,
    dying,
    dead,
};

pub const TransitionError = error{ SameState, InvalidTransition };

pub fn allowed(from: State, to: State) bool {
    return switch (from) {
        .invalid => to == .runnable,
        .runnable => to == .running or to == .dying,
        .running => to == .runnable or to == .blocked or to == .stopped or to == .dying,
        .blocked => to == .runnable,
        .stopped => to == .runnable or to == .dying,
        .dying => to == .dead,
        .dead => false,
    };
}

pub fn check(from: State, to: State) TransitionError!void {
    if (from == to) return error.SameState;
    if (!allowed(from, to)) return error.InvalidTransition;
}

pub fn isTerminal(s: State) bool {
    return s == .dying or s == .dead;
}

/// A thread may begin to block only while it is running (on a CPU).
pub fn canBlock(s: State) bool {
    return s == .running;
}

pub fn name(s: State) []const u8 {
    return @tagName(s);
}
