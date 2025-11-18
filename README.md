# r\_delta

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-v0.1.1--Alpha-yellow.svg)]()

**r\_delta** is a high-performance, memory-safe binary delta compression tool. It allows you to synchronize large files between systems by transferring *only* the modified bytes, not the entire file.

> **🚀 New in v0.1.1:** We have successfully migrated from Fixed-Size Chunking to **Content-Defined Chunking (FastCDC)**. This makes `r_delta` resistant to data shifts (insertions/deletions) and significantly improves deduplication rates for modified files.

## ⚡ Why r\_delta?

Most delta-transfer tools are either legacy C implementations (hard to maintain, memory unsafe) or tied to specific ecosystems. `r_delta` aims to be the modern standard:

* **Memory Safe:** Written in pure Rust.
* **Shift Resistant:** Uses Gear Hashing and FastCDC to align chunks based on content, not arbitrary offsets.
* **Fast:** BLAKE3 for cryptographic hashing + Gear Hash for rolling window detection.
* **Minimal Dependencies:** Only blake3, clap, and hex (no bloat).

## The Problem CDC Solves: Shift Resistance

### Legacy Fixed Chunking (rsync, older tools)
```
Source File:   [AAAA] [BBBB] [CCCC] [DDDD]

Insert 1 byte at start:
Target File:   [XAAA] [ABBB] [BCCC] [CDDD]
                       ↑ Everything shifted by 1 byte
Result: 0% deduplication. Must re-transmit entire file.
```

### r_delta with FastCDC (Content-Defined Chunking)
```
Source File:   [AAAA] [BBBB] [CCCC] [DDDD]

Insert 1 byte at start:
Target File:   [X] [AAAA] [BBBB] [CCCC] [DDDD]
                ↑ New chunk    ↑ Rest re-aligns to content boundaries
Result: ~99% deduplication. Only transmit the new byte + metadata.
```

**Why It Matters:** One byte insertion in the middle of a 1GB file used to require re-transmitting the entire file. Now only the delta is sent.

## 🛠 Installation

`r_delta` is not yet published to crates.io. To use it, build from source:

```bash
git clone https://github.com/abraham1003/r_delta
cd r_delta
cargo build --release
```

The binary will be located at `./target/release/r_delta`.

## 🚀 Usage

`r_delta` operates in three distinct phases: **Signature**, **Delta**, and **Patch**.

### 1\. Generate a Signature

Create a lightweight "map" of the original file using FastCDC.

```bash
./r_delta signature <OLD_FILE> <SIG_FILE>
```

### 2\. Calculate the Delta

Compare the new file against the signature to find changed blocks.

```bash
./r_delta delta <SIG_FILE> <NEW_FILE> <PATCH_FILE>
```

### 3\. Apply the Patch

Reconstruct the new file using the old file and the patch.

```bash
./r_delta patch <OLD_FILE> <PATCH_FILE> <RECREATED_FILE>
```

## 🗺 Roadmap & Architecture

We believe in building in public. Our CDC implementation uses industry-standard techniques found in rsync, Borg Backup, and restic.

### FastCDC Configuration (v0.1.1)
* **Min Chunk Size:** 2 KB (prevents metadata overhead)
* **Avg Chunk Size:** 8 KB (industry standard)
* **Max Chunk Size:** 64 KB (ensures deduplication efficiency)
* **Rolling Hash:** Gear Hash (256-entry lookup table, no expensive modulo operations)
* **Strong Hash:** BLAKE3 (SIMD-optimized, faster than SHA-256)

### Feature Status

| Feature | Status | Description |
| :--- | :---: | :--- |
| **Core Logic** | ✅ | Signature/delta/patch workflow |
| **FastCDC Engine** | ✅ | **v0.1.1.** Gear Hash + dynamic cut-point detection |
| **Shift Resistance** | ✅ | Handles insertions/deletions without full re-sync |
| **Compression** | ⬜ | Zstd/Lz4 for literal data |
| **Network Layer** | ⬜ | TCP/UDP protocol |
| **Multi-threading** | ⬜ | Parallel chunking for large files |

## 📄 License

Licensed under the Apache License, Version 2.0. See [LICENSE](./LICENSE) for details.
