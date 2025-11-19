# r\_delta

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-v0.1.2--Alpha-yellow.svg)]()

**r\_delta** is a high-performance, memory-safe data transport engine. It synchronizes large files by combining **Content-Defined Chunking (CDC)** for deduplication with **Zstd Entropy Coding** for new data compression.

> **🚀 New in v0.1.2:** We have introduced a **Hybrid Engine**. `r_delta` now compresses non-matching literal data using **Zstd**, reducing patch sizes significantly for changed files. We also added a forensic `verify` command.

## ⚡ Why r\_delta?

Most delta tools solve only half the problem (deduplication). `r_delta` solves the whole transport layer:

* **Shift Resistant:** Uses FastCDC (Gear Hash) to align chunks based on content, not offsets.
* **Hybrid Efficiency:**
    * *Known Data:* Deduplicated via `COPY` instructions (O(1) HashMap lookup).
    * *New Data:* Compressed via `COMPRESSED_LITERAL` instructions (Zstd).
* **Memory Safe:** Streaming architecture with 8MB buffer limits prevents RAM spikes, regardless of file size.
* **Forensic Integrity:** Includes a bit-level verification tool to guarantee `Original == Recreated`.

## The Problem CDC Solves: Shift Resistance

### Legacy Fixed Chunking
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

`r_delta` is built from source.

```bash
git clone https://github.com/abraham1003/r_delta
cd r_delta
cargo build --release
```

The binary will be at `./target/release/r_delta`.

## 🚀 Usage

`r_delta` now operates in four phases: **Signature**, **Delta**, **Patch**, and **Verify**.

### 1\. Signature (The Map)

Generate a lightweight "fingerprint" map of the old file (approx 0.7% of file size).

```bash
./r_delta signature <OLD_FILE> <SIG_FILE>
```

### 2\. Delta (The Hybrid Engine)

Compare the new file against the signature. Matches are referenced; new data is compressed.

```bash
./r_delta delta <SIG_FILE> <NEW_FILE> <PATCH_FILE>
```

### 3\. Patch (The Reassembler)

Reconstruct the new file using the old file and the optimized patch.

```bash
./r_delta patch <OLD_FILE> <PATCH_FILE> <RECREATED_FILE>
```

### 4\. Verify (The Auditor)

**New in v0.1.2:** Perform a high-speed, streaming bit-for-bit comparison to prove integrity.

```bash
./r_delta verify <ORIGINAL_FILE> <RECREATED_FILE>
```

## 🗺 Roadmap & Architecture

### Architecture Specs (v0.1.2)

* **Chunking:** FastCDC with Gear Hash (Avg 8KB chunks).
* **Hashing:** BLAKE3 (SIMD-optimized).
* **Compression:** Zstd (Level 3 default) for literals.
* **Protocol:** Custom Binary Format (`0x01 COPY`, `0x02 LITERAL`, `0x03 COMPRESSED`).
* **Safety:** 8MB Streaming Buffer for compression (Constant RAM usage).

### Feature Status

| Feature                | Status | Description                                  |
|:-----------------------|:------:|:---------------------------------------------|
| **FastCDC Engine**     |   ✅    | Gear Hash + dynamic cut-points               |
| **Shift Resistance**   |   ✅    | Handles insertions/deletions                 |
| **Hybrid Compression** |   ✅    | **v0.1.2** Zstd integration for literal runs |
| **Network Protocol**   |   🚧   | **Next Step:** Define `protocol.rs` structs  |
| **QUIC Transport**     |   ⬜    | Async UDP transport layer                    |
| **Remote Sync**        |   ⬜    | Client/Server architecture                   |

## 📄 License

Licensed under the Apache License, Version 2.0. See [LICENSE](./LICENSE) for details.