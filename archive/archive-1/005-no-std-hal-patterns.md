# 005 — `no_std` Patterns & HAL Design in Rust

**Task:** 5 — Research: `no_std` Patterns & HAL Design in Rust
**Depends on:** nothing (Wave 1, parallel)
**Feeds into:** Task 9 (HAL Trait Layer design)
**Status:** Complete

---

## Table of Contents

1. [The `no_std` + `alloc` Model](#1-the-no_std--alloc-model)
2. [The `embedded-hal` Pattern](#2-the-embedded-hal-pattern)
3. [The `embedded-storage` Crate — Most Directly Relevant](#3-the-embedded-storage-crate--most-directly-relevant)
4. [Case Studies: Crates That Do the Split Well](#4-case-studies-crates-that-do-the-split-well)
   - 4.1 `serde`
   - 4.2 `heapless`
   - 4.3 `smoltcp`
   - 4.4 `postcard` (bonus)
5. [Common Pitfalls](#5-common-pitfalls)
6. [Preliminary HAL Sketch for This Project](#6-preliminary-hal-sketch-for-this-project)
7. [Design Principles Summary](#7-design-principles-summary)
8. [Completion Report](#8-completion-report)

---

## 1. The `no_std` + `alloc` Model

### 1.1 What `no_std` Means

In a normal Rust crate, the compiler implicitly links the `std` crate, which provides:
- `Vec`, `String`, `HashMap`, `Box`, `Arc`, `Mutex`…
- File I/O, threads, environment access
- A panic handler
- A memory allocator
- The `std::error::Error` trait

`#![no_std]` at the crate root opts out of `std`. The crate then has access only to `core` — the subset of the standard library with no platform dependencies and no allocator:
- Primitive types, iterators, slices, `Option`, `Result`
- `fmt::Display`, `fmt::Debug` (but formatting without allocation is tricky)
- `core::ptr`, `core::mem`, `core::cell`
- **No** `Vec`, `String`, `Box`, `HashMap`

### 1.2 The `alloc` Crate

`alloc` is a middle tier: it requires a global allocator but not an OS. It provides:
- `Box<T>`, `Vec<T>`, `String`, `Arc<T>`, `Rc<T>`
- `BTreeMap`, `BTreeSet` (but not `HashMap` — that's in `std`)
- `alloc::collections`, `alloc::string`, `alloc::vec`

To use `alloc` in a `no_std` crate:

```rust
#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
```

On a bare-metal target, the consuming application must supply a global allocator:

```rust
// In the application binary (not the library):
use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();
```

On `std` targets, the standard allocator is supplied automatically.

### 1.3 The Three-Tier Architecture

Most well-designed `no_std` crates follow a three-tier model:

```
Tier 1: core only     — No allocator, no OS. Works everywhere.
         no_std, no alloc

Tier 2: core + alloc  — Requires a heap, but no OS.
         no_std, alloc feature

Tier 3: std           — Full standard library.
         std feature (often the default)
```

This maps directly to Cargo feature flags:

```toml
[features]
default = ["std"]
std = ["alloc"]     # std implies alloc
alloc = []          # alloc without std

[dependencies]
# no_std deps go here unconditionally
```

```rust
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;
```

### 1.4 `cfg_attr` vs Separate Crates

There are two structural approaches:

**Approach A — Feature flags in one crate** (most common, preferred):
- Single crate with `#![cfg_attr(not(feature = "std"), no_std)]`
- `std` feature enables `std`-specific impl blocks
- Core logic is always `no_std`-compatible

**Approach B — Split into `foo-core` and `foo` crates**:
- `foo-core` is `no_std` + alloc
- `foo` re-exports `foo-core` and adds `std` glue
- Used by some older or more complex crates
- Generally considered more overhead than it's worth today

**Recommendation for this project:** Feature flags in a single crate (Approach A). The `std` feature enables the persistent file-backed HAL backend. The `alloc` feature is always assumed (the database cannot work without heap allocation).

---

## 2. The `embedded-hal` Pattern

### 2.1 Overview

`embedded-hal` (https://github.com/rust-embedded/embedded-hal) is the canonical example of trait-based hardware abstraction in Rust. It defines traits for common hardware peripherals — SPI, I2C, GPIO, delays — so that device driver crates can be written once and work with any hardware implementation.

The central insight: **separate the driver from the hardware**. A device driver for an SPI flash chip depends on `embedded_hal::spi::SpiDevice`, not on any specific MCU's SPI peripheral. The MCU vendor provides an implementation of the trait; the driver works on all of them.

### 2.2 Design Principles of `embedded-hal`

**Associated error type:** Every trait method that can fail uses an associated `Error` type:

```rust
pub trait SpiDevice<Word: Copy + 'static = u8>: ErrorType {
    fn transaction(&mut self, operations: &mut [Operation<'_, Word>]) -> Result<(), Self::Error>;
    fn read(&mut self, buf: &mut [Word]) -> Result<(), Self::Error> { ... }
    fn write(&mut self, buf: &[Word]) -> Result<(), Self::Error> { ... }
}

pub trait ErrorType {
    type Error: Error;
}

pub trait Error: core::fmt::Debug {
    fn kind(&self) -> ErrorKind;
}
```

The `ErrorKind` enum provides a small vocabulary of error categories that drivers can match on, without knowing the concrete error type. This is the key to type-erased error handling in `no_std` — there is no `std::error::Error` (which requires `std`), so `embedded-hal` defines its own.

**Infallible operations:** When an operation truly cannot fail (e.g., an in-memory mock), `core::convert::Infallible` implements `Error`, so the associated type can be `Infallible`.

**Blocking vs non-blocking:** `embedded-hal` separates blocking and async variants into distinct traits/modules, giving users control over execution model.

**Sealed traits:** Older versions used the sealed trait pattern (a private `Sealed` supertrait) to prevent downstream crates from implementing HAL traits for arbitrary types. Version 1.0 moved away from this — the ecosystem found the openness more useful for testing and mocking.

### 2.3 What This Project Borrows from `embedded-hal`

| `embedded-hal` concept | This project's analogue |
|------------------------|-------------------------|
| `SpiDevice` trait | `StorageBackend` trait |
| `ErrorType` associated type | Our HAL's `Error` associated type |
| `ErrorKind` enum | `StorageErrorKind` enum |
| Impl for `Infallible` | In-memory backend: writes can't fail |
| Blocking operations | All our I/O is synchronous (blocking) |
| No `std` in core traits | No `std` in HAL trait definitions |

---

## 3. The `embedded-storage` Crate — Most Directly Relevant

### 3.1 Overview

`embedded-storage` (https://github.com/rust-embedded-community/embedded-storage) is specifically designed for storage media (NOR flash, EEPROM, SD cards) and is the closest existing analogue to what this project needs.

### 3.2 The Core Traits

```rust
/// Read from a storage medium at a given offset.
pub trait ReadStorage {
    type Error;
    /// Read `buf.len()` bytes starting at `offset` into `buf`.
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), Self::Error>;
    /// Total capacity in bytes.
    fn capacity(&self) -> usize;
}

/// Write to a storage medium. Note: does NOT imply erase.
pub trait WriteStorage: ReadStorage {
    fn write(&mut self, offset: u32, buf: &[u8]) -> Result<(), Self::Error>;
}

/// Combined read + write.
pub trait Storage: ReadStorage + WriteStorage {}

/// NOR flash with sector-erase semantics.
pub trait NorFlash: ReadStorage {
    const WRITE_SIZE: usize;    // minimum write granularity
    const ERASE_SIZE: usize;    // minimum erase granularity
    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error>;
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error>;
}
```

### 3.3 What This Project Needs Beyond `embedded-storage`

`embedded-storage` is designed for raw byte-addressable storage with fixed erase semantics. A database has additional requirements:

| Need | `embedded-storage` | This project |
|------|--------------------|--------------|
| Random read | ✓ (`read(offset, buf)`) | ✓ |
| Random write | ✓ (`write(offset, buf)`) | ✓ |
| Explicit sync/flush | ✗ | ✓ (fsync for durability) |
| Append/sequential write | ✗ | ✓ (WAL writes) |
| File open/create/truncate | ✗ | ✓ (std backend) |
| Capacity query | ✓ | ✓ |
| Seek/cursor model | ✗ | ✓ (page-oriented) |

This project will define its own HAL traits inspired by `embedded-storage` but extended for database-specific concerns. We do not depend on `embedded-storage` directly — our traits are a superset with different semantic contracts.

---

## 4. Case Studies: Crates That Do the Split Well

### 4.1 `serde` — The Gold Standard

**What it is:** The ubiquitous Rust serialization framework.

**How it splits:** `serde` is `no_std` compatible with no feature flags at all by default. It depends only on `core`. The `std` feature re-enables `std`-specific implementations (e.g., impls for `std::collections::HashMap`, `OsString`, `Path`).

```toml
# serde's Cargo.toml (simplified)
[features]
default = ["std"]
std = []
alloc = []
derive = ["serde_derive"]
```

```rust
// serde/src/lib.rs
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(all(feature = "alloc", not(feature = "std")))]
extern crate alloc;
```

**Key lessons:**
- The default is `std`, which keeps the ergonomic default for most users. Embedded users opt out.
- `alloc` and `std` are separate features. A crate can use `alloc` containers like `Vec` without pulling in `std`.
- All core serialization logic lives in `core` + `alloc`. The `std` feature only adds convenience impls.
- `serde` defines its own error trait that doesn't extend `std::error::Error`, making it `core`-compatible.

### 4.2 `heapless` — No Allocator Required

**What it is:** A library of data structures with static (compile-time) capacity: `heapless::Vec<T, N>`, `heapless::String<N>`, `heapless::FnvIndexMap<K, V, N>`.

**How it splits:** Pure `no_std`, no `alloc`. No feature flags needed. The data structures store their contents inline on the stack.

```rust
use heapless::Vec;

let mut v: Vec<u8, 64> = Vec::new(); // capacity 64, no heap
v.push(1).unwrap();
```

**Key lessons for this project:**
- Fixed-capacity structures are useful for metadata caches and registry tables in `no_std` contexts where allocation may be impossible.
- `heapless::FnvIndexMap` is a reasonable `no_std` alternative to `HashMap` for small, bounded registries (e.g., type name lookups).
- The "capacity as a const generic" pattern (`<N>`) enables fully stack-allocated types.
- **Trade-off:** Capacities are baked in at compile time. For a database with unbounded graph sizes, `alloc`-backed structures are necessary. `heapless` is useful only at the periphery (e.g., fixed-size buffers for I/O).

### 4.3 `smoltcp` — Full `no_std` Stack

**What it is:** A full TCP/IP network stack in pure Rust, designed for embedded systems. No OS, no POSIX, no allocator required (though it supports `alloc`-backed configurations too).

**Architecture:**
- `smoltcp` defines an `Interface` that consumes a `Device` trait for the physical network interface.
- The device trait has only two methods: `receive()` and `transmit()`, returning borrowed packet buffers.
- The network stack is entirely expressed as pure Rust logic on top of this minimal hardware trait.

**How it splits:**
```toml
[features]
default = ["std", "log", "medium-ethernet", "proto-ipv4", "proto-ipv6"]
std = ["managed/std"]
alloc = ["managed/alloc"]
# The "managed" dependency handles alloc vs static storage
```

The clever part is the `managed` crate, which provides `ManagedSlice<'a, T>` — a slice that can be either a `&'a mut [T]` (static, no allocator) or a `Vec<T>` (alloc). This enables callers to choose storage strategy:

```rust
// Static, no alloc:
let mut socket_storage = [SocketStorage::EMPTY; 8];
let iface = Interface::new(config, &mut device, smoltcp::time::Instant::ZERO);

// Dynamic, with alloc:
let sockets: Vec<SocketStorage> = Vec::new();
```

**Key lessons for this project:**
- The "managed storage" pattern (borrowed slice or owned vec, selectable at call site) is a powerful technique for types that need to be usable without an allocator.
- Keep the trait surface minimal — `smoltcp`'s `Device` trait has just two operations. The complexity lives in the implementation, not the trait.
- Feature flags should control which protocols/features are compiled, not the fundamental architecture.

### 4.4 `postcard` — `no_std` Serialization with I/O Flavors (Bonus)

**What it is:** A compact binary serialization format built on `serde`, designed for embedded.

**Why it's instructive:** `postcard` uses a "flavor" system to abstract over output targets:

```rust
pub trait Flavor {
    type Remainder: 'static;
    type Error: 'static;
    fn try_push(&mut self, byte: u8) -> Result<(), Self::Error>;
    fn finalize(self) -> Result<Self::Remainder, Self::Error>;
}

// Implementations:
// - Slice: writes into a &mut [u8]
// - Vec: writes into a Vec<u8> (alloc feature)
// - HVec: writes into a heapless::Vec (no alloc)
```

**Key lessons for this project:**
- The "flavor/backend" pattern is essentially what our HAL is: a trait that abstracts over the destination of bytes, allowing the same core logic to write to files, memory, flash, or custom I/O.
- Making the output target a type parameter (or trait object) rather than a hardcoded type is the correct architectural move.
- The same pattern appears in our project as `StorageBackend`: write page data to a file, a memory buffer, or a flash device, all via the same trait.

---

## 5. Common Pitfalls

### 5.1 Allocator Issues

**Problem: forgetting that `alloc` needs a global allocator**
Libraries that use `alloc` are fine — the requirement is placed on the binary or test crate that links them. But this surprises `no_std` newcomers who think adding `extern crate alloc` is enough.

**Problem: using `HashMap` in `no_std + alloc` code**
`HashMap` lives in `std`, not `alloc`. Use `BTreeMap` (from `alloc`) or `hashbrown::HashMap` (which is `no_std + alloc` compatible and is what `std::HashMap` wraps internally).

```rust
// WRONG in no_std:
use std::collections::HashMap;

// RIGHT in no_std + alloc:
use alloc::collections::BTreeMap;
// OR (with hashbrown dep):
use hashbrown::HashMap;
```

**Problem: `format!` and `println!` in `no_std`**
`format!` is available in `alloc` (it returns a `String`). `println!` is `std`-only. Use `alloc::format!` or `write!` with a custom writer.

### 5.2 Error Handling Without `std::error::Error`

`std::error::Error` has a blanket `impl` that requires `std`. In `no_std` contexts, you have several options:

**Option A: Define your own error trait** (what `embedded-hal` does)
```rust
// In core-compatible code:
pub trait StorageError: core::fmt::Debug {
    fn kind(&self) -> StorageErrorKind;
}
```

**Option B: Use `core::fmt::Display` + `core::fmt::Debug` as bounds** without an `Error` supertrait. Most practical for embedded.

**Option C: Use `anyhow` / `thiserror` with `no_std` feature flags** — `thiserror` works in `no_std` since v2.0. `anyhow` has a `no_std` mode with limited functionality.

**Option D: Use `snafu` with `no_std` support** — a structured error derivation library with `no_std` support.

**Recommendation for this project:** Define a concrete `StorageError` enum in the HAL crate. Use `thiserror` with `no_std` support for derivation. Provide `From` impls between layers. Avoid trait objects for errors in the hot path.

### 5.3 Trait Design Mistakes

**Mistake: overly generic traits that become unimplementable**

Every associated type and bound you add to a trait is a burden on every implementor. A HAL trait like:

```rust
// BAD: forces implementors to use async, DMA, specific error types
pub trait StorageBackend: AsyncRead + AsyncWrite + DmaCapable
where
    Self::Error: std::error::Error + Send + Sync + 'static,
```

...is impossible to implement on a bare-metal platform. Keep the trait surface minimal.

**Mistake: object-unsafe traits**

A trait is object-safe if it can be used as `dyn Trait`. Requirements for object safety:
- No methods with generic type parameters
- No associated constants (unless using `where Self: Sized`)
- No `where Self: Sized` methods (they're excluded from the vtable)
- `Self` not used in positions other than receiver

If your HAL traits use `fn read<const N: usize>(&mut self, ...)`, they are not object-safe. Prefer `fn read(&mut self, buf: &mut [u8])` (dynamic dispatch friendly).

For this project, we need `dyn StorageBackend` to allow runtime backend selection. **All HAL traits must be object-safe.**

**Mistake: associated type proliferation**

```rust
// BAD: hard to use, lots of boilerplate for callers
pub trait Storage {
    type ReadError;
    type WriteError;
    type FlushError;
    type SeekError;
}
```

Prefer a single associated `Error` type, with an `ErrorKind` enum for discrimination:

```rust
// GOOD
pub trait Storage {
    type Error: core::fmt::Debug;
}
```

**Mistake: not providing a `noop` or infallible implementation**

For testing and mocking, always ensure `Infallible` or a simple `VecBackend` can implement the trait with minimal boilerplate. If even a simple in-memory impl requires significant code, the trait is too complex.

### 5.4 Feature Flag Pitfalls

**Additive features:** Features must be strictly additive — enabling a feature must not break code that runs without it. Never use features to conditionally *remove* functionality.

**Feature flag proliferation:** Too many features make the crate hard to reason about. For this project, we need:
- `std` — default on, enables the persistent file-backed backend
- `alloc` — implied by `std`, enables heap-backed data structures in core code

That is likely sufficient. Avoid feature flags for individual subsystems.

**Test configuration:** When testing a `no_std` library, use:

```rust
#[cfg(test)]
mod tests {
    // Tests automatically run with std in the test harness
    // even if the library is no_std
    use super::*;
    
    #[test]
    fn test_something() { ... }
}
```

Integration tests in `tests/` always have `std` available. Use them for testing the `std` backend.

### 5.5 Panic Handling

In `no_std`, there is no default panic handler. The application must provide one:

```rust
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {} // or: abort, log, reset
}
```

Libraries should minimize panicking code. Use `Result` everywhere. Reserve `unwrap()` for cases that are logically impossible, and document why. For the database core, a panic in a write path is far worse than returning an error.

---

## 6. Preliminary HAL Sketch for This Project

This section produces the preliminary storage trait sketch requested in the task definition. This feeds directly into Task 9 (HAL Trait Layer design), which will refine these traits after the file format is known (Task 8).

### 6.1 Design Goals for the HAL

1. **Object-safe** — supports `dyn StorageBackend`
2. **`core`-compatible** — trait definitions have no `std` dependency
3. **Minimal surface** — implementable on constrained hardware
4. **Synchronous (blocking)** — no async complexity in v1
5. **Byte-addressable with explicit flush** — models both files and flash
6. **Configurable error type** — each backend defines its error

### 6.2 Core Traits

```rust
// hal/src/lib.rs
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

/// Categorizes storage errors for generic error handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageErrorKind {
    /// A read extended beyond the storage bounds.
    OutOfBounds,
    /// Underlying I/O failed (OS error, hardware fault, etc.).
    Io,
    /// The storage medium is read-only.
    ReadOnly,
    /// The storage is full (cannot grow).
    CapacityExceeded,
    /// Data corruption detected (checksum mismatch, etc.).
    Corruption,
    /// An operation was interrupted and may need retry.
    Interrupted,
    /// Any error not covered by the above variants.
    Other,
}

/// Base error trait for HAL errors. `no_std`-compatible.
/// Does not extend `std::error::Error`.
pub trait StorageError: core::fmt::Debug {
    fn kind(&self) -> StorageErrorKind;
}

/// Groups the error type for a storage implementation.
pub trait StorageErrorType {
    type Error: StorageError;
}

// ─── Read trait ──────────────────────────────────────────────────────────────

/// Random-access read from a storage medium.
pub trait ReadAt: StorageErrorType {
    /// Read bytes from `offset` into `buf`.
    ///
    /// Reads exactly `buf.len()` bytes. Returns an error if the read
    /// would extend beyond the end of the storage, or if an I/O
    /// failure occurs.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Returns the current size of the storage in bytes.
    fn size(&self) -> Result<u64, Self::Error>;
}

// ─── Write trait ─────────────────────────────────────────────────────────────

/// Random-access write to a storage medium.
pub trait WriteAt: StorageErrorType {
    /// Write `buf` at `offset`.
    ///
    /// Writes exactly `buf.len()` bytes. The implementation may buffer
    /// this write; call `flush()` to ensure durability.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Append `buf` to the end of the storage, growing it if needed.
    ///
    /// Returns the offset at which `buf` was written.
    fn append(&mut self, buf: &[u8]) -> Result<u64, Self::Error>;

    /// Truncate the storage to `new_size` bytes.
    ///
    /// If `new_size` is larger than the current size, the behavior
    /// is implementation-defined (some backends may zero-fill).
    fn set_size(&mut self, new_size: u64) -> Result<(), Self::Error>;
}

// ─── Flush/Sync trait ────────────────────────────────────────────────────────

/// Durability control: flush buffered writes to the underlying medium.
pub trait Flush: StorageErrorType {
    /// Flush all buffered writes.
    ///
    /// After this call returns `Ok(())`, all previous writes are
    /// guaranteed to be durable (i.e., they will survive a process
    /// crash). This maps to `fsync()` / `FlushFileBuffers()` on
    /// persistent backends, and is a no-op on in-memory backends.
    fn flush(&mut self) -> Result<(), Self::Error>;
}

// ─── Combined trait ──────────────────────────────────────────────────────────

/// Full storage backend: readable, writable, and flushable.
///
/// This is the primary trait the storage engine operates against.
/// All backends (persistent file, in-memory, custom) implement this.
pub trait StorageBackend: ReadAt + WriteAt + Flush {}

/// Blanket impl: anything that implements the three sub-traits
/// automatically implements `StorageBackend`.
impl<T: ReadAt + WriteAt + Flush> StorageBackend for T {}

// ─── Lifecycle trait ─────────────────────────────────────────────────────────

/// Open/create/close semantics for storage backends that need them.
///
/// Not all backends need this (an in-memory backend is always "open").
/// The std persistent backend implements this to manage file handles.
///
/// This trait is NOT part of `StorageBackend` to keep the core trait
/// minimal and no_std-friendly.
#[cfg(feature = "std")]
pub trait OpenableBackend: Sized {
    type Error: StorageError;
    type Config;

    fn open(config: Self::Config) -> Result<Self, Self::Error>;
    fn create(config: Self::Config) -> Result<Self, Self::Error>;
    fn close(self) -> Result<(), Self::Error>;
}
```

### 6.3 In-Memory Backend Sketch

```rust
// backends/memory.rs
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Error type for the in-memory backend.
#[derive(Debug, Clone, Copy)]
pub enum MemoryError {
    OutOfBounds { offset: u64, size: u64 },
    CapacityExceeded,
}

impl StorageError for MemoryError {
    fn kind(&self) -> StorageErrorKind {
        match self {
            MemoryError::OutOfBounds { .. } => StorageErrorKind::OutOfBounds,
            MemoryError::CapacityExceeded => StorageErrorKind::CapacityExceeded,
        }
    }
}

/// In-memory storage backend. No durability; writes are lost on drop.
///
/// Useful for testing, ephemeral databases, and environments without
/// persistent storage.
pub struct MemoryBackend {
    data: Vec<u8>,
}

impl StorageErrorType for MemoryBackend {
    type Error = MemoryError;
}

impl ReadAt for MemoryBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), MemoryError> {
        let offset = offset as usize;
        let end = offset.checked_add(buf.len()).ok_or(MemoryError::OutOfBounds {
            offset: offset as u64,
            size: self.data.len() as u64,
        })?;
        if end > self.data.len() {
            return Err(MemoryError::OutOfBounds {
                offset: offset as u64,
                size: self.data.len() as u64,
            });
        }
        buf.copy_from_slice(&self.data[offset..end]);
        Ok(())
    }

    fn size(&self) -> Result<u64, MemoryError> {
        Ok(self.data.len() as u64)
    }
}

impl WriteAt for MemoryBackend {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), MemoryError> {
        let offset = offset as usize;
        let end = offset + buf.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[offset..end].copy_from_slice(buf);
        Ok(())
    }

    fn append(&mut self, buf: &[u8]) -> Result<u64, MemoryError> {
        let offset = self.data.len() as u64;
        self.data.extend_from_slice(buf);
        Ok(offset)
    }

    fn set_size(&mut self, new_size: u64) -> Result<(), MemoryError> {
        self.data.resize(new_size as usize, 0);
        Ok(())
    }
}

impl Flush for MemoryBackend {
    /// No-op: in-memory writes are immediately visible.
    fn flush(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }
}
```

### 6.4 `std` Persistent Backend Sketch (outline only)

```rust
// backends/std_file.rs — requires `std` feature
use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};

pub struct FileBackend {
    file: File,
}

impl StorageErrorType for FileBackend {
    type Error = FileError;
}

impl ReadAt for FileBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), FileError> {
        // Use pread() on Unix, ReadFile() with offset on Windows
        // std::os::unix::fs::FileExt::read_exact_at
        // std::os::windows::fs::FileExt::seek_read
        todo!()
    }

    fn size(&self) -> Result<u64, FileError> {
        Ok(self.file.metadata()?.len())
    }
}

impl WriteAt for FileBackend {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), FileError> {
        // pwrite() / WriteFile() with offset
        todo!()
    }

    fn append(&mut self, buf: &[u8]) -> Result<u64, FileError> {
        let offset = self.size()?;
        self.write_at(offset, buf)?;
        Ok(offset)
    }

    fn set_size(&mut self, new_size: u64) -> Result<(), FileError> {
        self.file.set_len(new_size)?;
        Ok(())
    }
}

impl Flush for FileBackend {
    fn flush(&mut self) -> Result<(), FileError> {
        // fsync, not just fflush.
        // fflush only flushes userspace buffers to the OS.
        // fsync flushes OS buffers to the physical medium.
        self.file.sync_all()?; // maps to fsync() / FlushFileBuffers()
        Ok(())
    }
}
```

**Critical note:** `sync_all()` (which calls `fsync`) is orders of magnitude slower than `sync_data()` (which calls `fdatasync`) but also syncs metadata (file size, mtime). For WAL writes, `sync_data()` is usually sufficient. For the final commit, `sync_all()` may be required. Task 9 will determine the exact sync strategy based on the file format design from Task 8.

### 6.5 Hypothetical `no_std` Flash Backend (walkthrough)

To validate the trait design, here is how a bare-metal NOR flash backend would implement the traits using `embedded-storage`'s `NorFlash` trait:

```rust
// A wrapper that adapts embedded-storage's NorFlash to our StorageBackend.
pub struct NorFlashAdapter<F: embedded_storage::nor_flash::NorFlash> {
    flash: F,
    // NOR flash requires erase before write. We buffer writes here
    // to accumulate a full sector before erasing and rewriting.
    write_buffer: [u8; SECTOR_SIZE],
    dirty_sector: Option<u32>,
}

impl<F: NorFlash> StorageErrorType for NorFlashAdapter<F> {
    type Error = FlashAdapterError<F::Error>;
}

impl<F: NorFlash> ReadAt for NorFlashAdapter<F> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.flash.read(offset as u32, buf)
            .map_err(FlashAdapterError::Flash)
    }
    fn size(&self) -> Result<u64, Self::Error> {
        Ok(self.flash.capacity() as u64)
    }
}
// ... WriteAt and Flush with sector-buffering logic
```

This demonstrates that the trait surface is implementable on constrained hardware, validating the design.

---

## 7. Design Principles Summary

The following principles, derived from this research, should guide the HAL design in Task 9:

| Principle | Rationale |
|-----------|-----------|
| **Traits live in `core`; implementations live in feature-gated modules** | Separation of interface from implementation |
| **Single `Error` associated type per trait family** | Reduces boilerplate; `ErrorKind` enum handles discrimination |
| **All core traits must be object-safe** | Enables `dyn StorageBackend` for runtime dispatch |
| **`flush()` is a first-class operation** | Durability is explicit, not implicit |
| **`ReadAt` takes `&self`, `WriteAt` takes `&mut self`** | Allows concurrent reads with `Arc<Mutex<Backend>>` |
| **No async in HAL v1** | Async adds complexity; blocking I/O is sufficient for embedded single-threaded and OS-threaded use cases |
| **`Infallible` is a valid `Error` type** | In-memory backends can be error-free; simplifies testing |
| **Feature flags: `std` and `alloc` only** | Minimal surface; avoids combinatorial explosion |
| **Use `thiserror` with `no_std` support for error derivation** | Reduces boilerplate while staying portable |
| **`OpenableBackend` is `std`-only, separate from core trait** | Filesystem open/create has no analogue on bare metal |

---

## 8. Completion Report

### Summary of Findings

This document covers:
1. The `no_std` + `alloc` three-tier model and how to structure feature flags
2. The `embedded-hal` pattern — associated error types, `ErrorKind` discrimination, object safety
3. `embedded-storage` as the closest existing analogue — its traits and what this project needs beyond them
4. Three primary case studies (`serde`, `heapless`, `smoltcp`) plus a bonus (`postcard`), with extracted lessons
5. Common pitfalls: allocator requirements, `HashMap` vs `BTreeMap`, object safety, feature flag discipline, panic handling
6. A complete preliminary HAL trait sketch with `ReadAt`, `WriteAt`, `Flush`, `StorageBackend`, and error types
7. Implementations for the in-memory backend (complete) and `std` file backend (outline)
8. A walkthrough showing how a NOR flash adapter would implement the traits

### Key Decisions Informed by This Research

- **One crate, feature flags** (not `foo-core` + `foo` split): simpler dependency graph
- **`dyn StorageBackend` support is non-negotiable**: all traits must be object-safe
- **`flush()` must be explicit and separate**: maps to `fsync()`, not userspace buffer flush
- **`ReadAt` is `&self`**: enables concurrent reads across threads without `&mut`
- **No async in v1**: complexity is not justified for the use cases in scope

### Context for Next Task (Task 9 — HAL Trait Layer)

Task 9 (HAL Trait Layer design) depends on this document **and** Task 8 (file format). The preliminary sketch in section 6 is intentionally incomplete — Task 9 will refine it once the file format is known (e.g., the exact page size, whether the WAL is in the same file or a sidecar, and the precise sync requirements for crash safety).

Task 9 should read:
- This document (`005-no-std-hal-patterns.md`) — for the preliminary sketch and design principles
- `008-file-format-spec.md` (Task 8's deliverable) — for the exact I/O patterns the storage engine requires

### Residual Concerns

- **`pread`/`pwrite` portability:** On Unix, `read_at`/`write_at` can be implemented with `pread`/`pwrite` (thread-safe, no seek state). On Windows, the equivalent is more complex (`OVERLAPPED` structures or `ReadFile`/`WriteFile` with explicit offsets). The `std` backend must handle both. This is manageable but worth calling out for Task 9.
- **`sync_all` vs `sync_data`:** The correct fsync strategy (which operations need full `fsync` vs `fdatasync`) cannot be determined until the WAL design is settled in Task 8. Task 9 should explicitly address this.
- **Error type for cross-layer propagation:** The storage engine (Task 16) will have its own error type that wraps `StorageBackend::Error`. The conversion chain (HAL error → storage engine error → public API error) should be designed holistically in Task 9 or 12.

### Upstream Flags

**None.** All findings are consistent with the project's stated constraints and design philosophy.

---

*Document produced for Task 5. Feeds Task 9 directly; also informs Task 12 (design synthesis).*
