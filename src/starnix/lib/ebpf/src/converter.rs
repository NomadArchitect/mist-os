// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::{
    bpf_insn, sock_filter, BPF_A, BPF_ABS, BPF_ADD, BPF_ALU, BPF_AND, BPF_B, BPF_DIV, BPF_EXIT,
    BPF_H, BPF_IMM, BPF_IND, BPF_JA, BPF_JEQ, BPF_JGE, BPF_JGT, BPF_JLE, BPF_JLT, BPF_JMP,
    BPF_JMP32, BPF_JNE, BPF_JSET, BPF_K, BPF_LD, BPF_LDX, BPF_LSH, BPF_MEM, BPF_MISC, BPF_MOV,
    BPF_MUL, BPF_NEG, BPF_OR, BPF_RET, BPF_RSH, BPF_ST, BPF_STX, BPF_SUB, BPF_TAX, BPF_TXA, BPF_W,
    BPF_X, BPF_XOR,
};
use std::collections::HashMap;

use crate::visitor::Register;
use crate::EbpfError;
use crate::EbpfError::*;

const CBPF_WORD_SIZE: u32 = 4;

// cBPF supports 16 words for scratch memory.
const CBPF_SCRATCH_SIZE: u32 = 16;

// These are accessors for bits in an BPF/EBPF instruction.
// Instructions are encoded in one byte.  The first 3 LSB represent
// the operation, and the other bits represent various modifiers.
// Brief comments are given to indicate what the functions broadly
// represent, but for the gory detail, consult a detailed guide to
// BPF, like the one at https://docs.kernel.org/bpf/instruction-set.html

/// The bpf_class is the instruction type.(e.g., load/store/jump/ALU).
pub fn bpf_class(filter: &sock_filter) -> u32 {
    (filter.code & 0x07).into()
}

/// The bpf_size is the 4th and 5th bit of load and store
/// instructions.  It indicates the bit width of the load / store
/// target (8, 16, 32, 64 bits).
fn bpf_size(filter: &sock_filter) -> u32 {
    (filter.code & 0x18).into()
}

/// The addressing mode is the most significant three bits of load and
/// store instructions.  They indicate whether the instrution accesses a
/// constant, accesses from memory, or accesses from memory atomically.
pub fn bpf_addressing_mode(filter: &sock_filter) -> u32 {
    (filter.code & 0xe0).into()
}

/// Modifiers for jumps and alu operations.  For example, a jump can
/// be jeq, jtl, etc.  An ALU operation can be plus, minus, divide,
/// etc.
fn bpf_op(filter: &sock_filter) -> u32 {
    (filter.code & 0xf0).into()
}

/// The source for the operation (either a register or an immediate).
fn bpf_src(filter: &sock_filter) -> u32 {
    (filter.code & 0x08).into()
}

/// Similar to bpf_src, but also allows BPF_A - used for RET.
fn bpf_rval(filter: &sock_filter) -> u32 {
    (filter.code & 0x18).into()
}

/// Returns offset for the scratch memory with the specified address.
fn cbpf_scratch_offset(addr: u32) -> Result<i16, EbpfError> {
    if addr < CBPF_SCRATCH_SIZE {
        Ok((-(CBPF_SCRATCH_SIZE as i16) + addr as i16) * CBPF_WORD_SIZE as i16)
    } else {
        Err(EbpfError::InvalidCbpfScratchOffset(addr))
    }
}

fn new_bpf_insn(code: u32, dst: Register, src: Register, offset: i16, imm: i32) -> bpf_insn {
    bpf_insn {
        code: code as u8,
        _bitfield_1: linux_uapi::__BindgenBitfieldUnit::new([dst | src << 4]),
        off: offset,
        imm,
    }
}

/// Transforms a program in classic BPF (cbpf, as stored in struct
/// sock_filter) to extended BPF (as stored in struct bpf_insn).
/// The bpf_code parameter is kept as an array for easy transfer
/// via FFI.  This currently only allows the subset of BPF permitted
/// by seccomp(2).
pub(crate) fn cbpf_to_ebpf(bpf_code: &[sock_filter]) -> Result<Vec<bpf_insn>, EbpfError> {
    // There are only two BPF registers, A and X. There are 10
    // EBPF registers, numbered 0-9.  We map between the two as
    // follows:

    // r[0]: Mapped to A.
    // r[1]: ebpf makes this the memory passed in,
    // r[2]: ebpf makes this the length of the memory passed in.
    // r[9]: Mapped to X
    // r[10]: Const stack pointer. cBFP scratch memory (16 words) is stored on top of the stack.

    const REG_A: u8 = 0;
    const REG_X: u8 = 9;

    // Map from jump targets in the cbpf to a list of jump instructions in the epbf that target
    // it. When you figure out what the offset of the target is in the ebpf, you need to patch the
    // jump instructions to target it correctly.
    let mut to_be_patched: HashMap<usize, Vec<usize>> = HashMap::new();

    let mut ebpf_code: Vec<bpf_insn> = vec![];
    ebpf_code.reserve(bpf_code.len() * 2);

    for (i, bpf_instruction) in bpf_code.iter().enumerate() {
        // Update instructions processed previously that jump to the current one.
        if let Some((_, entries)) = to_be_patched.remove_entry(&i) {
            for index in entries {
                ebpf_code[index].off = (ebpf_code.len() - index - 1) as i16;
            }
        }

        // Helper to queue a new entry into `to_be_patched`.
        let mut prep_patch = |cbpf_offset: usize, ebpf_source: usize| -> Result<(), EbpfError> {
            let cbpf_target = i + 1 + cbpf_offset;
            if cbpf_target >= bpf_code.len() {
                return Err(EbpfError::InvalidCbpfJumpOffset(cbpf_offset as u32));
            }
            to_be_patched.entry(cbpf_target).or_insert_with(Vec::new).push(ebpf_source);
            Ok(())
        };

        match bpf_class(bpf_instruction) {
            BPF_ALU => match bpf_op(bpf_instruction) {
                BPF_ADD | BPF_SUB | BPF_MUL | BPF_DIV | BPF_AND | BPF_OR | BPF_XOR | BPF_LSH
                | BPF_RSH => {
                    let e_instr = if bpf_src(bpf_instruction) == BPF_K {
                        new_bpf_insn(
                            bpf_instruction.code as u32,
                            REG_A,
                            0,
                            0,
                            bpf_instruction.k as i32,
                        )
                    } else {
                        new_bpf_insn(bpf_instruction.code as u32, REG_A, REG_X, 0, 0)
                    };
                    ebpf_code.push(e_instr);
                }
                BPF_NEG => {
                    ebpf_code.push(new_bpf_insn(BPF_ALU | BPF_NEG, REG_A, REG_A, 0, 0));
                }
                _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
            },
            class @ (BPF_LD | BPF_LDX) => {
                let dst_reg = if class == BPF_LDX { REG_X } else { REG_A };

                let mode = bpf_addressing_mode(bpf_instruction);
                let size = bpf_size(bpf_instruction);

                // Half-word (`BPF_H`) and byte (`BPF_B`) loads are allowed only for
                // `BPF_LD | BPD_ABS` and `BPF_LD | BPD_IND`. All other loads should be word-sized
                // (i.e. `BPF_W`).
                match (size, mode, class) {
                    (BPF_W, _, _) => (),
                    (BPF_H | BPF_B, BPF_ABS | BPF_IND, BPF_LD) => (),
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                };

                match mode {
                    BPF_ABS => {
                        // TODO(b/42079971): This should use `BPF_LD | BPF_ABS | size`.
                        ebpf_code.push(new_bpf_insn(
                            BPF_LDX | BPF_MEM | size,
                            dst_reg,
                            1,
                            bpf_instruction.k as i16,
                            0,
                        ));
                    }
                    BPF_IMM => {
                        let imm = bpf_instruction.k as i32;
                        ebpf_code.push(new_bpf_insn(BPF_LDX | BPF_IMM, dst_reg, 0, 0, imm));
                    }
                    BPF_MEM => {
                        // cBPF's scratch memory is stored in the stack referenced by R10.
                        let offset = cbpf_scratch_offset(bpf_instruction.k)?;
                        ebpf_code.push(new_bpf_insn(BPF_LDX | BPF_MEM, dst_reg, 10, offset, 0));
                    }
                    //  TODO(b/42079971): Add `BPF_LEN`.
                    //  TODO(b/42079971): Add `BPF_IND`.
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                }
            }
            BPF_JMP => {
                match bpf_op(bpf_instruction) {
                    BPF_JA => {
                        ebpf_code.push(new_bpf_insn(BPF_JMP | BPF_JA, 0, 0, -1, 0));
                        prep_patch(bpf_instruction.k as usize, ebpf_code.len() - 1)?;
                    }
                    op @ (BPF_JGT | BPF_JGE | BPF_JEQ | BPF_JSET) => {
                        // In cBPD, JMPs have a jump-if-true and jump-if-false branch. eBPF only
                        // has jump-if-true. In most cases only one of the two branches actually
                        // jumps (the other one is set to 0). In these cases the instruction can
                        // be translated to 1 eBPF instruction. Otherwise two instructions are
                        // produced in the output.

                        let src = bpf_src(bpf_instruction);
                        let sock_filter { k, jt, jf, .. } = *bpf_instruction;
                        let (src_reg, imm) = if src == BPF_K { (0, k as i32) } else { (REG_X, 0) };

                        // When jumping only for the false case we can negate the comparison
                        // operator to achieve the same effect with a single jump-if-true eBPF
                        // instruction. That doesn't work for `BPF_JSET`. It is handled below
                        // using 2 instructions.
                        if jt == 0 && op != BPF_JSET {
                            let op = match op {
                                BPF_JGT => BPF_JLE,
                                BPF_JGE => BPF_JLT,
                                BPF_JEQ => BPF_JNE,
                                _ => panic!("Unexpected operation: {op:?}"),
                            };

                            ebpf_code.push(new_bpf_insn(
                                BPF_JMP32 | op | src,
                                REG_A,
                                src_reg,
                                -1,
                                imm,
                            ));
                            prep_patch(jf as usize, ebpf_code.len() - 1)?;
                        } else {
                            // Jump if true.
                            ebpf_code.push(new_bpf_insn(
                                BPF_JMP32 | op | src,
                                REG_A,
                                src_reg,
                                -1,
                                imm,
                            ));
                            prep_patch(jt as usize, ebpf_code.len() - 1)?;

                            // Jump if false. Jumps with 0 offset are no-op and can be omitted.
                            if jf > 0 {
                                ebpf_code.push(new_bpf_insn(BPF_JMP | BPF_JA, 0, 0, -1, 0));
                                prep_patch(jf as usize, ebpf_code.len() - 1)?;
                            }
                        }
                    }
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                }
            }
            BPF_MISC => match bpf_op(bpf_instruction) {
                BPF_TAX => {
                    ebpf_code.push(new_bpf_insn(BPF_ALU | BPF_MOV | BPF_X, REG_X, REG_A, 0, 0));
                }
                BPF_TXA => {
                    ebpf_code.push(new_bpf_insn(BPF_ALU | BPF_MOV | BPF_X, REG_A, REG_X, 0, 0));
                }
                _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
            },

            class @ (BPF_ST | BPF_STX) => {
                if bpf_addressing_mode(bpf_instruction) != 0 || bpf_size(bpf_instruction) != 0 {
                    return Err(InvalidCbpfInstruction(bpf_instruction.code));
                }

                // cBPF's scratch memory is stored in the stack referenced by R10.
                let src_reg = if class == BPF_STX { REG_X } else { REG_A };
                let offset = cbpf_scratch_offset(bpf_instruction.k)?;
                ebpf_code.push(new_bpf_insn(BPF_STX | BPF_MEM | BPF_W, 10, src_reg, offset, 0));
            }
            BPF_RET => {
                match bpf_rval(bpf_instruction) {
                    BPF_K => {
                        // We're returning a particular value instead of the contents of the
                        // return register, so load that value into the return register.
                        let imm = bpf_instruction.k as i32;
                        ebpf_code.push(new_bpf_insn(BPF_ALU | BPF_MOV | BPF_IMM, REG_A, 0, 0, imm));
                    }
                    BPF_A => (),
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                };

                ebpf_code.push(new_bpf_insn(BPF_JMP | BPF_EXIT, 0, 0, 0, 0));
            }
            _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
        }
    }

    assert!(to_be_patched.is_empty());

    Ok(ebpf_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cbpf_to_ebpf() {
        // Jump to the next instruction.
        assert_eq!(
            cbpf_to_ebpf(&vec![
                sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 0 },
                sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
            ]),
            Ok(vec![
                new_bpf_insn(BPF_JMP | BPF_JA, 0, 0, 0, 0),
                new_bpf_insn(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
            ]),
        );

        // Jump after last instruction.
        assert_eq!(
            cbpf_to_ebpf(&vec![
                sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 1 },
                sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
            ]),
            Err(EbpfError::InvalidCbpfJumpOffset(1)),
        );

        // Jump out of bounds.
        assert_eq!(
            cbpf_to_ebpf(&vec![sock_filter {
                code: (BPF_JMP | BPF_JA) as u16,
                jt: 0,
                jf: 0,
                k: 0xffffffff
            }]),
            Err(EbpfError::InvalidCbpfJumpOffset(0xffffffff)),
        );

        // BPF_JNE is allowed only in eBPF.
        assert_eq!(
            cbpf_to_ebpf(&vec![
                sock_filter { code: (BPF_JMP | BPF_JNE) as u16, jt: 0, jf: 0, k: 0 },
                sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
            ]),
            Err(EbpfError::InvalidCbpfInstruction((BPF_JMP | BPF_JNE) as u16)),
        );

        // BPF_JEQ is supported in BPF.
        assert_eq!(
            cbpf_to_ebpf(&vec![
                sock_filter { code: (BPF_JMP | BPF_JEQ) as u16, jt: 1, jf: 0, k: 0 },
                sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
                sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
            ]),
            Ok(vec![
                new_bpf_insn(BPF_JMP32 | BPF_JEQ, 0, 0, 1, 0),
                new_bpf_insn(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
                new_bpf_insn(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
            ]),
        );

        // Make sure the jump is translated correctly when the jump target produces 2 instructions.
        assert_eq!(
            cbpf_to_ebpf(&vec![
                sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 0 },
                sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k: 1 },
            ]),
            Ok(vec![
                new_bpf_insn(BPF_JMP | BPF_JA, 0, 0, 0, 0),
                new_bpf_insn(BPF_ALU | BPF_MOV | BPF_IMM, 0, 0, 0, 1),
                new_bpf_insn(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
            ]),
        );

        // BPF_MEM access.
        assert_eq!(
            cbpf_to_ebpf(&vec![
                sock_filter { code: (BPF_LD | BPF_MEM) as u16, jt: 0, jf: 0, k: 0 },
                sock_filter { code: (BPF_LDX | BPF_MEM) as u16, jt: 0, jf: 0, k: 15 },
                sock_filter { code: BPF_ST as u16, jt: 0, jf: 0, k: 0 },
                sock_filter { code: BPF_STX as u16, jt: 0, jf: 0, k: 15 },
            ]),
            Ok(vec![
                new_bpf_insn(BPF_LDX | BPF_MEM | BPF_W, 0, 10, -64, 0),
                new_bpf_insn(BPF_LDX | BPF_MEM | BPF_W, 9, 10, -4, 0),
                new_bpf_insn(BPF_STX | BPF_MEM | BPF_W, 10, 0, -64, 0),
                new_bpf_insn(BPF_STX | BPF_MEM | BPF_W, 10, 9, -4, 0),
            ]),
        );

        // BPF_MEM access out of bounds.
        assert_eq!(
            cbpf_to_ebpf(&vec![sock_filter {
                code: (BPF_LD | BPF_MEM) as u16,
                jt: 0,
                jf: 0,
                k: 17,
            }]),
            Err(EbpfError::InvalidCbpfScratchOffset(17)),
        );
    }
}
