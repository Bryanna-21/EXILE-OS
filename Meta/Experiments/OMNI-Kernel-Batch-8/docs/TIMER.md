# Timer and interrupt controller

`src/timer.rs` computes the programming sequences as data so they can be tested: `divisor_for(hz)` (rejects 0 and rates
the PIT cannot make), `pic_sequence` (ICW1 to ICW4 with the offsets 0x20 and 0x28 and given masks), `pit_sequence`
(channel 0, lobyte/hibyte, rate generator mode 2).

Kernel test: remap the 8259 PICs, unmask only IRQ0, program the PIT to 100 Hz, `sti`, spin with `pause` (bounded at
200,000,000 iterations) until five ticks arrive, `cli`, mask everything. Pass line `OMNI8 TIMER: ticks ok`; failure
`OMNI8 FAIL: timer produced no ticks`. Timer interrupts arrive on IST5.

Spurious IRQ7 and IRQ15 are acknowledged and counted. Sending EOI for a spurious IRQ7 can in principle clear a real
in-service interrupt; harmless here because only IRQ0 is unmasked.

Not implemented: LAPIC/IOAPIC, HPET/TSC calibration, SMP, timer-driven scheduling.

Status: arithmetic and sequences unit-tested; the interrupt itself **has not been delivered by a CPU or emulator**.
