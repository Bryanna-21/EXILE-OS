# EXILE Kernel Validation Orchestrator

`exile_validate.py` runs the kernel validation ladder in one command and writes JSON + Markdown
reports and a SHA-256 manifest. Standard library only. It never installs or changes toolchains,
never edits tracked files (checked at the end by `integrity.tracked-files`), and writes only under `--out`.
It does not modify or replace any existing script; it calls them (`build.sh`, `check_image.py`,
`check_asm.py`, `qemu_test.py --selftest`, `kernel_syntax_check.sh`) and imports the Batch 8
transcript matcher (`qemu_test.compare/parse_expected`) rather than duplicating it.

## Run

```
python3 Meta/Exile/validation/exile_validate.py --out /path/to/out
python3 Meta/Exile/validation/exile_validate.py --list-stages
python3 Meta/Exile/validation/exile_validate.py --only env,build.base,image.base,iso.base,boot.base
python3 Meta/Exile/validation/exile_validate.py --kernel-syntax full --deny-warnings --timeout 120
python3 -I -B Meta/Exile/validation/tests/test_exile_validate.py     # orchestrator's own tests
```

Env overrides: `EXILE_VALIDATE_QEMU`, `EXILE_VALIDATE_GRUB_MKRESCUE`, `ZIG`.
Exit codes: 0 PASS, 3 BLOCKED/INCOMPLETE, 4 FAIL, 2 usage.

Needed for a full run: pinned rustc (`toolchain.pin`) with target `x86_64-unknown-none`,
`qemu-system-x86_64` >= 6.2, `grub-mkrescue`, `xorriso`, zig 0.15.2, clang. Missing tools give
BLOCKED, never PASS.

## Stages and evidence classes

environment < static < host-test < build < **runtime**. Only `boot.*` stages are runtime evidence:
QEMU launched the GRUB ISO, serial was captured to a file, the process exited via isa-debug-exit
(33 pass, 35 expected-fault), the transcript matched `tests/expected/<variant>.txt`, with no
timeout, panic, triple fault or unexpected reset. `summary.runtime_validated` is true only when all
seven variants pass that way; the report also records the QEMU path, version and binary SHA-256.

`env.*`, `selftest.qemu-matcher`, `host.b7..b11.{fmt,clippy,test}`, `host.euk.tests`,
`static.kernel-syntax`, `build.<variant>` (7) + `build.reproducible`, `image.<variant>` (own
ELF64/Multiboot1 validator + existing checkers), `iso.<variant>`, `boot.<variant>`,
`zig.regression`, `integrity.tracked-files`.

## Output

`<out>/{logs,artifacts/<variant>,reports,work}`, `reports/validation-report.{json,md}`,
`MANIFEST.sha256` (all of logs/artifacts/reports; `work/` scratch is excluded).
Per-boot: `serial.txt`, `qemu-debug.log`, milestone table (reached/missing) in the report.

## Known limitations

- Reset / triple-fault detection patterns (`CPU Reset`, `Triple fault` in QEMU's `-d cpu_reset` log)
  are unit-tested on synthetic logs only; they have not been validated against a real QEMU here.
- The tests use fake qemu/grub to exercise control flow; they prove classification logic, not boots.
- Builds with a non-pinned rustc are labelled `host-fallback`; clippy results under such a compiler
  carry a note and must be re-checked under the pin (newer lints can differ).
- `static.kernel-syntax` defaults to the scheduler translation units; `--kernel-syntax full` takes minutes.
- The ISO layout/QEMU args come from the Batch 8 harness conventions (`qemu_test.QEMU_ARGS`, `boot/grub.cfg`),
  with serial redirected to a file; the kernel is placed at `/boot/omni8.elf`, matching `boot/grub.cfg`.
