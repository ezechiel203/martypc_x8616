# 80286 simulation — staged model (spec-sheet path, no hardware)

This documents the 286 modelling effort in `x8616_cycles`, what is implemented,
and the honest boundary between what spec sheets can give and what needs the die.

## Layer 0 — functional execution (DONE)

The 80286 *real-mode integer ISA* is identical to the 80186 ISA for everything
the X8616 backend emits (ENTER/LEAVE, PUSH imm, IMUL imm, immediate-count
shifts, INS/OUTS, BOUND, PUSHA/POPA, plus the base 8086 set). MartyPC's NEC
Vx0 (V30) core already executes that set correctly on a 16-bit bus, so `--cpu
286` runs the program functionally on the V30 engine. Verified: MT16 produces
byte-identical output (same 1024 bytes, checksum 0x9d) on V20, V30, and 286.
(286-only real-mode instructions — LGDT/LIDT/LMSW/SMSW — and protected mode are
not needed for the toolchain and are deferred.)

## Layer 1 — datasheet instruction timing (DONE, validated)

`timing286.rs` encodes the 80286 datasheet "Instruction Set Clock Count
Summary": per-instruction clock counts for the no-wait-state, word-aligned
case, keyed by operation and operand kind (register / memory / immediate), with
the data-dependent forms (shift counts, branch direction via FLAGS, REP/LOOP
counts via CX) resolved from live CPU state at each instruction. Salient 286
characteristics captured:

- **Free effective address.** The Address Unit computes EAs in parallel, so
  there is no 8086-style 5–12 cycle EA penalty; the datasheet memory-form counts
  already include the access.
- **Hardware multiply/divide.** `MUL r16`=21, `DIV r16`=22 vs the 8086's
  ~118–133 — the dominant speedup on compute code.
- **2-cycle register ALU**, immediate-count shifts (5+n), `+m` prefetch reload
  on taken transfers.

Validated to the cycle on a controlled sequence: `MOV rr(2)+ADD rr(2)+IMUL
r16(21)+SHL imm4(9)+MOV ri(2)+HLT(2) = 38` → model reports exactly 38. On MT16
the model reports 158,410 cycles vs the V30's 525,448 bus cycles — the expected
~3.3× from the hardware multiplier and fast shifts.

Accuracy class: **instruction-level, datasheet-exact** for the covered classes
(which cover the whole X8616 instruction mix). It is NOT pipeline-cycle-exact —
it sums per-instruction datasheet counts and does not model cycle-by-cycle bus
contention or fetch/execute overlap stalls.

## Layer 2 — microarchitecture (spec-derived blueprint; the gate-level bound)

The 80286 datasheet + hardware reference describe four overlapped units, and the
die floorplan (public die photos) shows them as distinct blocks:

- **BU — Bus Unit**: the bus state machine (address pipelining, Ts/Tc states,
  the 2-clock bus cycle, prefetcher, the 6-byte queue) and READY handshake.
- **IU — Instruction Unit**: decodes prefetched bytes into a 3-deep decoded-
  instruction queue.
- **EU — Execution Unit**: the ALU, register file, microcode engine.
- **AU — Address Unit**: segment + offset addition, the descriptor cache, limit
  checks (the source of "free EA").

A **cycle-exact** model (Stage 3) implements these as a clocked state machine:
the BU bus cycle drives prefetch, the IU/EU overlap is what makes back-to-back
register ops issue at 2 clocks, and stalls appear when the EU outruns the queue.
The datasheet timing of Layer 1 is the *steady-state* projection of that machine
and is the correctness oracle for it.

### Where spec sheets stop

A **gate-level** model (transistors/netlist, à la visual6502/perfect6502) cannot
be built from spec sheets — they contain no gate data. It requires the 286 die
extracted to a netlist (decap → die photography → polygon/transistor tracing).
No public 286 netlist exists. So from documentation alone the deepest faithful
layer is the **microarchitecture/cycle model** above; gate-level remains gated on
die extraction (or the hardware validator of Stage 2-with-hardware, which is the
empirical substitute for a netlist when validating timing).

## Summary

| layer | what | status |
|-------|------|--------|
| 0 functional ISA | real-mode 186/286 exec (V30 engine) | done, byte-validated |
| 1 datasheet timing | per-instruction 286 clock counts | done, cycle-validated |
| 2 microarchitecture | BU/IU/EU/AU clocked pipeline | blueprint (this doc) → Stage 3 |
| 3 gate-level | transistor netlist | needs die extraction |
