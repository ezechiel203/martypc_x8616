//! Local CI for the martypc_x8616 fork.
//!
//! `cargo xtask ci` runs the checks that matter for our contribution (the
//! `x8616_cycles` cycle ticker + the V20/V30/286 work) without dragging in the
//! whole eframe/wgpu desktop build:
//!
//!   1. rustfmt --check on our crates
//!   2. clippy on x8616_cycles (skipped by --fast)
//!   3. release build of the x8616-cycles ticker
//!   4. functional smoke tests: run known machine-code fixtures through the
//!      ticker on several CPU models and assert the cycle/exit results
//!      (regression guard for the cores and the 80286 timing model).
//!
//! Usage: `cargo xtask ci` | `cargo xtask ci --fast` | `cargo xtask smoke`

use std::{path::PathBuf, process::Command};

const OURS: &[&str] = &["x8616_cycles", "xtask"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("ci");
    let fast = args.iter().any(|a| a == "--fast");

    let ok = match cmd {
        "ci" => run_ci(fast),
        "smoke" => run_smoke(),
        other => {
            eprintln!("unknown xtask: {other} (want: ci [--fast] | smoke)");
            false
        }
    };
    if !ok {
        std::process::exit(1);
    }
}

fn workspace_root() -> PathBuf {
    // crates/xtask/.. /.. = workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .expect("workspace root")
}

/// Run a command, stream its output, return whether it succeeded.
fn step(label: &str, cmd: &mut Command) -> bool {
    println!("\n=== {label} ===");
    match cmd.status() {
        Ok(s) if s.success() => {
            println!("  PASS: {label}");
            true
        }
        Ok(s) => {
            println!("  FAIL: {label} (exit {:?})", s.code());
            false
        }
        Err(e) => {
            println!("  FAIL: {label} ({e})");
            false
        }
    }
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn run_ci(fast: bool) -> bool {
    let root = workspace_root();
    let mut all = true;

    // 1. rustfmt --check on our crates only (marty_core is vendored upstream).
    let mut fmt = cargo();
    fmt.current_dir(&root).arg("fmt").arg("--check");
    for p in OURS {
        fmt.arg("-p").arg(p);
    }
    all &= step("rustfmt --check (ours)", &mut fmt);

    // 2. clippy on x8616_cycles (slow — pulls marty_core; skipped by --fast).
    if !fast {
        // --no-deps: lint ONLY our crate, not the vendored upstream deps (whose
        // own clippy warnings must not gate our CI).
        let mut clippy = cargo();
        clippy
            .current_dir(&root)
            .args(["clippy", "-p", "x8616_cycles", "--no-deps", "--", "-D", "warnings"]);
        all &= step("clippy x8616_cycles (--no-deps)", &mut clippy);
    }
    else {
        println!("\n=== clippy x8616_cycles === (skipped: --fast)");
    }

    // 3. release build of the ticker.
    let mut build = cargo();
    build
        .current_dir(&root)
        .args(["build", "--release", "-p", "x8616_cycles"]);
    all &= step("build --release x8616_cycles", &mut build);

    // 4. functional smoke tests against the built ticker.
    all &= run_smoke();

    println!(
        "\n========== xtask ci: {} ==========",
        if all { "PASS" } else { "FAIL" }
    );
    all
}

/// A smoke fixture: machine code, the CPU model to run it on, and an assertion
/// on the ticker's `key=value` output line.
struct Fixture {
    name:   &'static str,
    code:   &'static [u8],
    cpu:    &'static str,
    expect: fn(&str) -> Result<(), String>,
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
}

fn run_smoke() -> bool {
    let root = workspace_root();
    let ticker = root.join("target/release/x8616-cycles");
    if !ticker.exists() {
        // Build it if a bare `xtask smoke` was invoked.
        let mut b = cargo();
        b.current_dir(&root).args(["build", "--release", "-p", "x8616_cycles"]);
        if !step("build ticker for smoke", &mut b) {
            return false;
        }
    }

    let fixtures = [
        // 8086: `mov ax, 0x1234; hlt` — base ISA runs on the 8086 core.
        Fixture {
            name:   "8086 mov/hlt",
            code:   &[0xB8, 0x34, 0x12, 0xF4],
            cpu:    "8086",
            expect: |o| {
                if field(o, "halted") != Some("1") {
                    return Err("did not halt".into());
                }
                if field(o, "ax") != Some("0x1234") {
                    return Err(format!("ax != 0x1234 ({o})"));
                }
                Ok(())
            },
        },
        // 286 timing model: MOV rr(2)+ADD rr(2)+IMUL r16(21)+SHL imm4(9)
        //                   +MOV ri(2)+HLT(2) = 38 (datasheet).
        Fixture {
            name:   "286 timing = 38",
            code:   &[
                0x89, 0xd8, 0x01, 0xc8, 0xf7, 0xe9, 0xc1, 0xe0, 0x04, 0xba, 0xf4, 0x00, 0xf4,
            ],
            cpu:    "286",
            expect: |o| {
                if field(o, "halted") != Some("1") {
                    return Err("did not halt".into());
                }
                match field(o, "cycles") {
                    Some("38") => Ok(()),
                    other => Err(format!("286 cycles = {other:?}, expected 38 ({o})")),
                }
            },
        },
        // V30 runs the same 186-ISA program functionally (16-bit bus).
        Fixture {
            name:   "v30 runs 186 ISA",
            code:   &[
                0x89, 0xd8, 0x01, 0xc8, 0xf7, 0xe9, 0xc1, 0xe0, 0x04, 0xba, 0xf4, 0x00, 0xf4,
            ],
            cpu:    "v30",
            expect: |o| {
                if field(o, "halted") != Some("1") {
                    return Err(format!("v30 did not halt ({o})"));
                }
                Ok(())
            },
        },
    ];

    println!("\n=== smoke tests ===");
    let mut all = true;
    let tmp = std::env::temp_dir();
    for (i, f) in fixtures.iter().enumerate() {
        let bin = tmp.join(format!("xtask_smoke_{i}.bin"));
        if std::fs::write(&bin, f.code).is_err() {
            println!("  FAIL: {} (write fixture)", f.name);
            all = false;
            continue;
        }
        let out = Command::new(&ticker)
            .args([
                bin.to_str().unwrap(),
                "--load",
                "0x7e00",
                "--entry",
                "0x7e00",
                "--cpu",
                f.cpu,
            ])
            .output();
        match out {
            Ok(o) => {
                let line = String::from_utf8_lossy(&o.stdout);
                let line = line.lines().next().unwrap_or("");
                match (f.expect)(line) {
                    Ok(()) => println!("  PASS: {}", f.name),
                    Err(e) => {
                        println!("  FAIL: {} — {e}", f.name);
                        all = false;
                    }
                }
            }
            Err(e) => {
                println!("  FAIL: {} ({e})", f.name);
                all = false;
            }
        }
    }
    all
}
