// x8616-cycles — a cycle-accurate T-state ticker for x86_16 (X8616) programs.
//
// Loads a flat binary into a MartyPC Intel 8086/8088 core (cycle-accurate,
// hardware-validated against real silicon), runs it from a given entry point
// until the program executes HLT, and prints the exact number of CPU cycles
// (T-states) spent — the optimization objective for the X8616 backend, the
// x86_16 analogue of the early-8085 project's cycle ticker.
//
// No ROMs / BIOS / machine config required: the cpu core ships a flat 1 MiB RAM
// bus, so we just write the program bytes in, point CS:IP at the entry, and run.
//
// Usage:
//   x8616-cycles <flat.bin> [options]
//     --load   <hex>   linear load address of the binary   (default 0x7E00)
//     --entry  <hex>   linear entry address (CS=0, IP=entry) (default = load)
//     --cpu    8086|8088                                     (default 8086)
//     --sp     <hex>   initial SP                            (default 0x7C00)
//     --max    <dec>   cycle cap before declaring a hang     (default 50_000_000)
//     --quiet          print only the cycle count
//
// Output (default): one line of `key=value` fields, e.g.
//   cycles=12345 insns=2001 al=0x17 ax=0x0017 ip=0x7e0c halted=1

mod timing286;

use anyhow::{bail, Context, Result};
use marty_core::bytequeue::ByteQueue; // brings `seek` into scope for decode
use marty_core::{
    cpu_808x::Cpu,
    cpu_common::{builder::CpuBuilder, calc_linear_address, CpuAddress, CpuError, CpuType, Register16, Register8},
};

struct Args {
    bin: String,
    load: usize,
    entry: usize,
    cpu: CpuType,
    sp: u16,
    max: u64,
    quiet: bool,
    // 80286 datasheet-timing estimate: execute functionally on the V30 (shared
    // real-mode 186/286 ISA) but accumulate cycles from the 286 timing model.
    est286: bool,
    dump: Option<(usize, usize, Option<String>)>, // (addr, len, optional raw-out file)
}

fn parse_num(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).with_context(|| format!("bad hex: {s}"))
    }
    else {
        s.parse::<u64>()
            .or_else(|_| u64::from_str_radix(s, 16))
            .with_context(|| format!("bad number: {s}"))
    }
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        bin: String::new(),
        load: 0x7E00,
        entry: usize::MAX,
        cpu: CpuType::Intel8086,
        sp: 0x7C00,
        max: 50_000_000,
        quiet: false,
        est286: false,
        dump: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--load" => a.load = parse_num(&it.next().context("--load needs a value")?)? as usize,
            "--entry" => a.entry = parse_num(&it.next().context("--entry needs a value")?)? as usize,
            "--sp" => a.sp = parse_num(&it.next().context("--sp needs a value")?)? as u16,
            "--max" => a.max = parse_num(&it.next().context("--max needs a value")?)?,
            "--quiet" => a.quiet = true,
            "--dump" => {
                // ADDR:LEN[:rawfile] — after HLT, read LEN bytes at ADDR.
                let spec = it.next().context("--dump needs ADDR:LEN[:file]")?;
                let mut parts = spec.splitn(3, ':');
                let addr = parse_num(parts.next().context("--dump addr")?)? as usize;
                let len = parse_num(parts.next().context("--dump len")?)? as usize;
                let file = parts.next().map(|s| s.to_string());
                a.dump = Some((addr, len, file));
            }
            "--cpu" => {
                a.cpu = match it.next().context("--cpu needs a value")?.as_str() {
                    "8086" => CpuType::Intel8086,
                    "8088" => CpuType::Intel8088,
                    // NEC V20 implements the 80186 instruction superset (ENTER,
                    // immediate-count shifts, 3-operand IMUL, PUSH imm, ...) — the
                    // ops our `-mcpu=80286` codegen emits. MartyPC has no 80286,
                    // so the V20 is the part that actually runs 186/286-tier code
                    // (8-bit bus, so cycles run higher than a real 16-bit 286).
                    "v20" => CpuType::NecV20(Default::default()),
                    // V30: 16-bit external bus + 186 ISA — the closest part in
                    // MartyPC to a real-mode 286 (the 286's 16-bit bus, minus the
                    // 286's faster microcode/pipeline). Now enabled in the V30 core.
                    "v30" => CpuType::NecV30(Default::default()),
                    // 80286 real mode: no 286 core exists in MartyPC, so execute
                    // functionally on the V30 (the 186/286 real-mode integer ISA
                    // is identical for our codegen) and report the 80286 datasheet
                    // timing model instead of the V30's bus cycles.
                    "286" => {
                        a.est286 = true;
                        CpuType::NecV30(Default::default())
                    }
                    other => bail!("unknown cpu: {other} (want 8086, 8088, v20, v30, or 286)"),
                }
            }
            other if a.bin.is_empty() && !other.starts_with("--") => a.bin = other.to_string(),
            other => bail!("unexpected argument: {other}"),
        }
    }
    if a.bin.is_empty() {
        bail!("usage: x8616-cycles <flat.bin> [--load hex] [--entry hex] [--cpu 8086|8088] [--sp hex] [--max N] [--quiet]");
    }
    if a.entry == usize::MAX {
        a.entry = a.load;
    }
    Ok(a)
}

fn main() -> Result<()> {
    let args = parse_args()?;

    let image = std::fs::read(&args.bin).with_context(|| format!("reading {}", args.bin))?;
    if args.load + image.len() > 0x10_0000 {
        bail!(
            "program ({} bytes @ {:#x}) overflows the 1 MiB address space",
            image.len(),
            args.load
        );
    }
    if args.entry >= 0x10_0000 {
        bail!("entry {:#x} outside address space", args.entry);
    }

    // Build a bare cycle-accurate CPU with its default flat 1 MiB RAM bus.
    let mut cpu = CpuBuilder::new()
        .with_cpu_type(args.cpu)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build CPU: {e}"))?;

    // CS=0 so linear address == offset; point IP at the entry via the reset vector
    // (reset() latches CS:IP from it and zeroes the rest of the register file).
    cpu.set_reset_vector(CpuAddress::Segmented(0x0000, args.entry as u16));
    cpu.reset();

    // Load the program image into RAM.
    for (i, &byte) in image.iter().enumerate() {
        cpu.bus_mut()
            .write_u8(args.load + i, byte, 0)
            .map_err(|e| anyhow::anyhow!("write {:#x}: {e:?}", args.load + i))?;
    }

    // A sane initial stack; real entry stubs (_start) typically reload SS:SP anyway.
    cpu.set_register16(Register16::SS, 0x0000);
    cpu.set_register16(Register16::SP, args.sp);
    cpu.set_register16(Register16::DS, 0x0000);
    cpu.set_register16(Register16::ES, 0x0000);

    // Run until the program executes HLT. On the 8086/8088, HLT with interrupts
    // disabled (our post-reset state, since no entry stub re-enables them) can
    // never resume — the MartyPC core surfaces this as `CpuHaltedError`, which
    // is precisely our "program terminated" signal, not a fault. The HLT's own
    // execution cycles are already counted when this returns.
    let mut halted = false;
    let mut cyc286: u64 = 0;
    loop {
        // 286 estimate: decode the instruction at CS:IP with the CPU's own
        // decoder (peek — no state change) and add its 80286 datasheet cost,
        // using live FLAGS/CL/CX for the data-dependent forms.
        if args.est286 {
            let cs = cpu.get_register16(Register16::CS);
            let ip = cpu.get_ip();
            let flags = cpu.get_flags();
            let cl = cpu.get_register8(Register8::CL);
            let cx = cpu.get_register16(Register16::CX);
            let lin = calc_linear_address(cs, ip) as usize;
            cpu.bus_mut().seek(lin);
            let ct = cpu.get_type();
            if let Ok(instr) = ct.decode(cpu.bus_mut(), true) {
                cyc286 += timing286::cycles_286(&instr, flags, cl, cx) as u64;
            }
        }
        match cpu.step(false) {
            Ok(_) => {
                let _ = cpu.step_finish(None);
            }
            Err(CpuError::CpuHaltedError(_)) => {
                halted = true;
                break;
            }
            Err(e) => return Err(anyhow::anyhow!("cpu step error: {e}")),
        }
        let (total, halt) = cpu.get_cycle_ct();
        if halt > 0 {
            halted = true; // also handle HLT-with-interrupts-enabled, just in case
            break;
        }
        if total > args.max {
            break; // hang / runaway — report what we have, halted=0
        }
    }

    let (total, halt) = cpu.get_cycle_ct();
    let v30_exec = total.saturating_sub(halt); // exclude the HLT spin cycles
    let insns = cpu.get_instruction_ct();
    let al = cpu.get_register8(Register8::AL);
    let ax = cpu.get_register16(Register16::AX);
    let ip = cpu.get_ip();
    // In 286 mode the reported cycles are the datasheet-timing estimate; the
    // V30 bus cycles are still shown for reference.
    let exec = if args.est286 { cyc286 } else { v30_exec };

    if args.quiet {
        println!("{exec}");
    }
    else if args.est286 {
        println!(
            "cycles={exec} insns={insns} al=0x{al:02x} ax=0x{ax:04x} ip=0x{ip:04x} halted={} \
             model=80286-datasheet (v30_bus_cycles={v30_exec})",
            halted as u8
        );
    }
    else {
        println!(
            "cycles={exec} insns={insns} al=0x{al:02x} ax=0x{ax:04x} ip=0x{ip:04x} halted={}",
            halted as u8
        );
    }

    // Optional memory dump: read the program's output buffer straight out of RAM.
    if let Some((addr, len, file)) = &args.dump {
        let mut bytes = Vec::with_capacity(*len);
        for off in 0..*len {
            let (b, _) = cpu
                .bus_mut()
                .read_u8(addr + off, 0)
                .map_err(|e| anyhow::anyhow!("read {:#x}: {e:?}", addr + off))?;
            bytes.push(b);
        }
        if let Some(path) = file {
            std::fs::write(path, &bytes).with_context(|| format!("writing {path}"))?;
            eprintln!("dumped {} bytes from {:#x} to {}", len, addr, path);
        }
        else {
            // 16 bytes per line of hex.
            for (i, chunk) in bytes.chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                println!("{:04x}: {}", addr + i * 16, hex.join(" "));
            }
        }
    }

    if !halted {
        std::process::exit(2); // ticker hit the cycle cap without halting
    }
    Ok(())
}
