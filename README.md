# dynarmic-sys-mythrax

Rust bindings for the [Dynarmic](https://github.com/lioncash/dynarmic) ARM dynamic recompiler — **Mythrax fork**, with Windows MSVC support.

> **What this fork adds over upstream `dynarmic-sys` 0.1.2:**
>
> 1. **Builds on Windows MSVC.** The vendored `mman.h` / `mman.c` shim is extended with `MAP_NORESERVE`, `MADV_WILLNEED`, and a `madvise()` no-op stub so the wrapper code compiles cleanly under cl.exe.
> 2. **Correct MSVC link line.** `build.rs` now emits `cargo:rustc-link-lib=static={fmt,mcl,Zydis,Zycore}` on MSVC (upstream only emitted them on GNU/Apple), fixing `LNK2001`/`LNK2019` unresolved-external errors against the bundled `fmt::v10` / Zydis symbols.
>
> Both fixes are minimal and intended to be upstreamed; this crate exists so projects depending on dynarmic-sys can build today on Windows without waiting for the merge.

## Credits

- **Original Project**: [Dynarmic](https://github.com/lioncash/dynarmic) by [lioncash](https://github.com/lioncash)
- **Initial Implementation Reference**: [rnidbg](https://github.com/fuqiuluo/rnidbg) by [fuqiuluo](https://github.com/fuqiuluo)
- **Upstream `dynarmic-sys`**: [wyourname](https://github.com/wyourname/dynarmic-sys)
- **Windows fork (this crate)**: [Mythrax-ZS](https://github.com/Mythrax-ZS/dynarmic-sys)

## Features

- High-level safe(r) wrapper for ARM32 and ARM64 emulation.
- Integrated C++ source (vendored) for easier building.
- Support for custom memory mapping and protection.
- Support for SVC and Unmapped memory callbacks.
- **Builds out-of-the-box on Linux, macOS, and Windows MSVC.**

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
dynarmic-sys-mythrax = "0.2"
```

If you want to drop-in replace upstream `dynarmic-sys` without code changes, alias it:

```toml
[dependencies]
dynarmic-sys = { package = "dynarmic-sys-mythrax", version = "0.2" }
```

## Build requirements

- A C++17 compiler (cl.exe on MSVC; clang/gcc on Linux/macOS).
- CMake ≥ 3.12 and Ninja on PATH.
- **Boost headers ≥ 1.57** discoverable via `find_package(Boost)` (e.g. `BOOST_ROOT=C:\local\boost_1_83_0` on Windows).

On Windows MSVC, if your environment includes devkitPro's MSYS2 cmake, force the real one + bundled Ninja:

```powershell
$env:CMAKE = "C:\Program Files\CMake\bin\cmake.exe"
$env:PATH = "C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;" + $env:PATH
$env:BOOST_ROOT = "C:\local\boost_1_83_0"
cargo build
```

## Example

```rust
use dynarmic_sys_mythrax::Dynarmic;

fn main() -> anyhow::Result<()> {
    let emu: Dynarmic<()> = Dynarmic::new();
    // see examples/basic_a64.rs for a full demo
    Ok(())
}
```

## Configuration

- `DYNARMIC_JIT_SIZE` — JIT cache size in MB (default: 64).

## License

0BSD, matching upstream Dynarmic.
