# Rust XDK SDK Work Handoff (Paused)

Date: 2026-02-12

## Status

Rust SDK generation work inside `/Users/mkieffer/programming/x-monitor/xdk` is **partially implemented and paused**.

For now, the main Rust TUI should continue using the direct X integration already added in the app (this path is currently working better than the generated SDK path).

## What Was Implemented

### 1. New Rust target in `xdk-gen`

Added Rust language wiring:

- `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/src/lib.rs`
- `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/src/rust/mod.rs`
- `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/src/rust/generator.rs`

Generator includes:

- `language!` setup for Rust
- casing rules
- per-tag renders (`models`, `client_module`, `client_class`)
- singleton renders (`lib`, `main_client`, `http_client`, `error`, `cargo_toml`, `readme`, `gitignore`)
- test renders (`test_operations`, `test_smoke`)

### 2. Rust template set

Added templates under:

- `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/templates/rust/`

Files:

- `cargo_toml.j2`
- `client_class.j2`
- `client_module.j2`
- `error.j2`
- `gitignore.j2`
- `http_client.j2`
- `lib.j2`
- `main_client.j2`
- `models.j2`
- `readme.j2`
- `test_operations.j2`
- `test_smoke.j2`

### 3. `xdk-build` command support

Added Rust generation command support:

- `/Users/mkieffer/programming/x-monitor/xdk/xdk-build/src/main.rs`
- `/Users/mkieffer/programming/x-monitor/xdk/xdk-build/src/rust.rs`

CLI path added:

- `cargo run -p xdk-build -- rust --spec <spec.json> --output <dir>`

### 4. Version config

Added Rust version key:

- `/Users/mkieffer/programming/x-monitor/xdk/xdk-config.toml`
- `[versions] rust = "0.1.0"`

### 5. Generator fixes applied

Two fixes were applied during debugging:

- Added `[workspace]` to generated crate template to avoid parent workspace nesting conflicts:
  - `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/templates/rust/cargo_toml.j2`
- Added `.rs` detection in template header logic so generated Rust files use `//` comments:
  - `/Users/mkieffer/programming/x-monitor/xdk/xdk-lib/src/templates.rs`

## Current Known Issues

### 1. Generated output still stale relative to latest template edit

The template now uses uppercase HTTP methods (`Method::GET`, etc.), but generation was interrupted before re-verifying a clean output.

- Template changed:
  - `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/templates/rust/client_class.j2`
- Current generated files in `/Users/mkieffer/programming/x-monitor/xdk/xdk/rust` still show lowercase methods (`Method::get`), which fail to compile.

### 2. Path parameter replacement token rendering bug

Generated code currently shows:

- `format!("{}", key)`

where it should include braces for path parameters (e.g. `{id}`), so replacement/filter logic is incorrect for templated URL segments.

This comes from Rust template escaping around curly braces in `client_class.j2`.

### 3. Network-limited validation in this environment

`cargo check` on the generated crate may fail here if crates cannot be fetched (DNS/network restrictions), even after codegen issues are fixed.

## Last successful generation command used

From `/Users/mkieffer/programming/x-monitor/xdk`:

```bash
cargo run -p xdk-build -- rust --spec latest-openapi.json --output xdk/rust
```

## Resume Checklist

1. Fix brace escaping in `/Users/mkieffer/programming/x-monitor/xdk/xdk-gen/templates/rust/client_class.j2` so `{param}` replacement logic renders correctly.
2. Re-run generation command above.
3. Verify generated methods are uppercase (`Method::GET`, `Method::POST`, etc.).
4. Run in generated crate:
   - `cargo fmt`
   - `cargo check`
5. Add small integration tests against representative endpoints (path params, query params, JSON body).
6. Only after generated SDK is stable, evaluate replacing direct X calls in the main TUI.

## Decision Recorded

Until this resume checklist is complete, use the existing direct Rust TUI X integration path (current working approach) instead of the generated Rust SDK.
