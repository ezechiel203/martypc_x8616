//! Intel 80286 real-mode instruction-timing model, derived from the published
//! 80286 datasheet ("Instruction Set Clock Count Summary"), not from hardware.
//!
//! This is the spec-sheet layer of the 286 simulation: the 286 computes the
//! effective address in its Address Unit in parallel (EA is "free", unlike the
//! 8086's 5–12 cycle EA), has a hardware multiplier (MUL/DIV are ~5× faster than
//! the 8086), and overlaps fetch/decode/execute across its 4 units (BU/IU/EU/AU).
//! The datasheet clock counts already fold the AU/bus overlap into per-instruction
//! numbers for the common, no-wait-state, word-aligned case — which is what we
//! reproduce here. The only modeled fudge is `M`, the datasheet's post-transfer
//! prefetch-reload term (bytes refetched after a control transfer); on the 16-bit
//! bus that is ~one word fetch, approximated as a small constant.
//!
//! Accuracy class: instruction-level, datasheet-exact for the covered classes
//! (which cover the entire X8616 codegen instruction mix). It does NOT model the
//! pipeline cycle-by-cycle or bus contention — that is Stage 3 (cycle-exact),
//! validated against real silicon. Uncovered opcodes fall back to a documented
//! default rather than silently reading zero.

use marty_core::cpu_common::{instruction::Instruction, operands::OperandType, Mnemonic};

/// Post-transfer prefetch-queue reload penalty (datasheet "m"), 16-bit bus.
const M: u32 = 3;
/// Conservative default for any opcode not explicitly modeled below.
const DEFAULT: u32 = 4;

#[inline]
fn is_mem(op: OperandType) -> bool {
    matches!(
        op,
        OperandType::AddressingMode(..)
            | OperandType::Offset8(_)
            | OperandType::Offset16(_)
            | OperandType::M16Pair(..)
            | OperandType::FarAddress(..)
    )
}

#[inline]
fn is_imm(op: OperandType) -> bool {
    matches!(
        op,
        OperandType::Immediate8(_) | OperandType::Immediate16(_) | OperandType::Immediate8s(_)
    )
}

/// Evaluate an x86 condition code (the low nibble of a 7x Jcc opcode) against
/// FLAGS, to decide the taken/not-taken timing of a conditional branch.
fn cond_taken(cc: u8, flags: u16) -> bool {
    let cf = flags & 0x0001 != 0;
    let pf = flags & 0x0004 != 0;
    let zf = flags & 0x0040 != 0;
    let sf = flags & 0x0080 != 0;
    let of = flags & 0x0800 != 0;
    match cc & 0x0F {
        0x0 => of,                // JO
        0x1 => !of,               // JNO
        0x2 => cf,                // JB/JC
        0x3 => !cf,               // JAE/JNC
        0x4 => zf,                // JZ/JE
        0x5 => !zf,               // JNZ/JNE
        0x6 => cf || zf,          // JBE
        0x7 => !(cf || zf),       // JA
        0x8 => sf,                // JS
        0x9 => !sf,               // JNS
        0xA => pf,                // JP
        0xB => !pf,               // JNP
        0xC => sf != of,          // JL
        0xD => sf == of,          // JGE
        0xE => zf || (sf != of),  // JLE
        _ => !(zf || (sf != of)), // JG
    }
}

/// 80286 cycle count for one instruction, given the live FLAGS / CL / CX needed
/// for data-dependent forms (shift counts, branch direction, REP/LOOP counts).
pub fn cycles_286(instr: &Instruction, flags: u16, cl: u8, cx: u16) -> u32 {
    use Mnemonic::*;
    let op1 = instr.operand1_type;
    let op2 = instr.operand2_type;
    let mem = is_mem(op1) || is_mem(op2);
    let word = matches!(op1, OperandType::Register16(_)) || matches!(op2, OperandType::Register16(_));

    match instr.mnemonic {
        // --- data movement -------------------------------------------------
        MOV => {
            if is_mem(op1) {
                3 // mem,reg / mem,imm
            }
            else if is_mem(op2) {
                5 // reg,mem
            }
            else {
                2 // reg,reg / reg,imm / reg,sreg
            }
        }
        LEA => 3,
        XCHG => {
            if mem {
                5
            }
            else {
                3
            }
        }
        PUSH => {
            if is_mem(op1) {
                5
            }
            else {
                3
            }
        }
        POP => 5, // POP reg and POP mem are both 5 on the 286
        PUSHF | PUSHA => 3,
        POPF => 5,
        POPA => 19,
        LDS | LES => 7,
        CBW | CWD => 2,

        // --- ALU (ADD/OR/ADC/SBB/AND/SUB/XOR) ------------------------------
        ADD | OR | ADC | SBB | AND | SUB | XOR => {
            if mem {
                7 // any memory operand: mem,reg / reg,mem / mem,imm
            }
            else if is_imm(op2) {
                3 // reg,imm
            }
            else {
                2 // reg,reg
            }
        }
        CMP => {
            if mem {
                if is_mem(op1) && is_imm(op2) {
                    6
                }
                else {
                    7
                }
            }
            else if is_imm(op2) {
                3
            }
            else {
                2
            }
        }
        TEST => {
            if mem {
                6
            }
            else if is_imm(op2) {
                3
            }
            else {
                2
            }
        }
        INC | DEC | NEG | NOT => {
            if mem {
                7
            }
            else {
                2
            }
        }

        // --- shifts / rotates: reg,1=2; reg,(CL|imm)=5+n; mem forms +EA -----
        SHL | SHR | SAR | ROL | ROR | RCL | RCR => {
            // count source from opcode: D0/D1=by 1, D2/D3=by CL, C0/C1=by imm8
            let (base, n) = match instr.opcode {
                0xD0 | 0xD1 => (if mem { 7 } else { 2 }, 0u32),
                0xD2 | 0xD3 => (if mem { 8 } else { 5 }, cl as u32),
                0xC0 | 0xC1 => (
                    if mem { 8 } else { 5 },
                    match op2 {
                        OperandType::Immediate8(v) => v as u32,
                        OperandType::Immediate8s(v) => (v as u8) as u32,
                        _ => 1,
                    },
                ),
                _ => (if mem { 7 } else { 2 }, 0),
            };
            base + n
        }

        // --- multiply / divide (hardware multiplier — the 286's big win) ----
        MUL => {
            if word {
                if mem {
                    24
                }
                else {
                    21
                }
            }
            else if mem {
                16
            }
            else {
                13
            }
        }
        IMUL => {
            if word {
                if mem {
                    24
                }
                else {
                    21
                }
            }
            else if mem {
                16
            }
            else {
                13
            }
        }
        DIV => {
            if word {
                if mem {
                    25
                }
                else {
                    22
                }
            }
            else if mem {
                17
            }
            else {
                14
            }
        }
        IDIV => {
            if word {
                if mem {
                    28
                }
                else {
                    25
                }
            }
            else if mem {
                20
            }
            else {
                17
            }
        }

        // --- control transfer (datasheet "+m" prefetch reload on taken) -----
        JMP => match instr.opcode {
            0xEB | 0xE9 => 7 + M, // near direct
            0xFF => {
                if mem {
                    11 + M
                }
                else {
                    7 + M
                }
            } // near indirect
            0xEA => 11 + M,       // far direct
            _ => 7 + M,
        },
        CALL => match instr.opcode {
            0xE8 => 7 + M,
            0xFF => {
                if mem {
                    11 + M
                }
                else {
                    7 + M
                }
            }
            0x9A => 13 + M,
            _ => 7 + M,
        },
        RETN => 11 + M,
        RETF => 15 + M,
        LOOP => {
            if cx != 1 {
                8 + M
            }
            else {
                4
            }
        } // taken while CX (pre-dec) != 1
        LOOPE => {
            if cx != 1 && (flags & 0x40 != 0) {
                8 + M
            }
            else {
                4
            }
        }
        LOOPNE => {
            if cx != 1 && (flags & 0x40 == 0) {
                8 + M
            }
            else {
                4
            }
        }
        JCXZ => {
            if cx == 0 {
                8 + M
            }
            else {
                4
            }
        }
        INT => 23 + M,
        INT3 => 23 + M,
        IRET => 17 + M,

        // --- string ops (per the datasheet base; REP handled by step count) -
        MOVSB | MOVSW => 5,
        STOSB | STOSW => 3,
        LODSB | LODSW => 5,
        SCASB | SCASW => 7,
        CMPSB | CMPSW => 8,

        // --- 186/286 stack frame & misc ------------------------------------
        ENTER => 11, // level 0 (our codegen uses ENTER size, 0)
        LEAVE => 5,
        NOP => 3,
        HLT => 2,
        CLC | STC | CMC | CLD | STD | CLI | STI => 2,
        IN => 5,
        OUT => 3,

        // Conditional branches (7x): taken 7+m, not taken 3.
        _ if (0x70..=0x7F).contains(&instr.opcode) => {
            if cond_taken(instr.opcode, flags) {
                7 + M
            }
            else {
                3
            }
        }

        _ => DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    // Spot-checks against the published 80286 datasheet clock counts.
    // (Constructed Instructions are awkward here; these document the intent and
    //  the canonical numbers the table above must reproduce.)
    // MOV reg,reg = 2 | MOV reg,mem = 5 | MOV mem,reg = 3
    // ALU reg,reg = 2 | reg,imm = 3 | reg,mem = 7
    // MUL r16 = 21 | DIV r16 = 22 | IMUL r16 = 21 | IDIV r16 = 25
    // PUSH reg = 3 | POP reg = 5 | LEA = 3 | LEAVE = 5 | ENTER l0 = 11
    // Jcc taken = 7+m | not taken = 3
}
