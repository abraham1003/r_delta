# r\_delta

[![License](https://img.shields.io/badge/License-Apache_2.0_%2F_BSL_1.1-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-v0.1.2--Alpha-yellow.svg)]()

**r\_delta** is a high-performance, data transport engine that achieves **99%+ bandwidth savings** on incremental updates. It combines **Content-Defined Chunking (CDC)** for shift-resistant deduplication with **Zstd compression** and **SKIP optimization** for maximum efficiency.

> **🚀 New in v0.1.2** 
> - ✅ **Directory Synchronization**: Sync entire directories with smart diff algorithm
> - ✅ **Manifest Generation**: Fast directory walking with `.gitignore` support (ignore crate)
> - ✅ **Hybrid Compression**: Zstd integration for optimal patch sizes
> - ✅ **Network Sync**: Full QUIC-based client-server architecture with cryptographic verification
> - ✅ **Professional UX**: Real-time progress bars, spinners, and deduplication reports
> - ✅ **SKIP Optimization**: Additional 5% patch size reduction for large sequential regions
> - ✅ **Telemetry**: Structured logging with performance metrics (throughput, duration, savings)
> - ✅ **Forensic Verification**: Bit-level integrity checking
> - ✅ **Content-Based Checksums**: BLAKE3 hashing for reliable file change detection
> - ✅ **Smart Diff Algorithm**: O(n log n) manifest comparison with SendFull, SendDelta, Skip, Delete actions
> - ✅ **File Deletion Handling**: Removes server files not in client manifest
> - ✅ **Sync Planning**: Protocol extensions for manifest exchange and multi-file coordination

## ⚡ Why r\_delta?

Most delta tools solve only half the problem (deduplication). `r_delta` solves the whole transport layer:

* **Shift Resistant:** Uses FastCDC (Gear Hash) to align chunks based on content, not offsets.
* **Hybrid Efficiency:**
    * *Known Data:* Deduplicated via `COPY` instructions (O(1) HashMap lookup).
    * *New Data:* Compressed via `COMPRESSED_LITERAL` instructions (Zstd level 3).
    * *Sequential Data:* Optimized via `SKIP` instructions (~5% metadata reduction).
* **Memory Safe:** Streaming architecture with 8MB buffer limits prevents RAM spikes, regardless of file size.
* **Professional UX:** Real-time progress bars, spinners, and deduplication reports show exactly what's happening.
* **Telemetry:** Structured logging with performance metrics (throughput, duration, savings).
* **Forensic Integrity:** Bit-level verification tool guarantees `Original == Recreated`.
* **Network Ready:** QUIC-based transport with automatic delta detection and cryptographic verification.

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

`r_delta` is must be built from source using Cargo workspaces.

```bash
git clone https://github.com/abraham1003/r_delta
cd r_delta
cargo build --release
```

The binaries will be compiled to:
- `./target/release/r_delta` (client CLI)
- `./target/release/r_delta_server` (server daemon)

**Note:** The CLI tool is not yet published to crates.io.

### Workspace Structure

The repository is structured as a workspace with three crates:
- **`crates/core`** - The core library (Apache 2.0) 
- **`crates/client`** - The CLI tool (Apache 2.0)
- **`crates/server`** - The server component (BSL 1.1)

## 🚀 Usage

`r_delta` operates in five phases: **Signature**, **Delta**, **Patch**, **Verify**, and **Sync**.

After building, use the compiled binaries directly from `./target/release/`:

### 1\. Signature (The Map)

Generate a lightweight "fingerprint" map of the old file (approx 0.7% of file size).

```bash
./target/release/r_delta signature <OLD_FILE> <SIG_FILE>
```

**Example:**
```bash
./target/release/r_delta signature old_version.bin old.sig
```

### 2\. Delta (The Hybrid Engine)

Compare the new file against the signature. Matches are referenced; new data is compressed.

```bash
./target/release/r_delta delta <SIG_FILE> <NEW_FILE> <PATCH_FILE>
```

**Example:**
```bash
./target/release/r_delta delta old.sig new_version.bin patch.bin
```

### 3\. Patch (The Reassembler)

Reconstruct the new file using the old file and the optimized patch.

```bash
./target/release/r_delta patch <OLD_FILE> <PATCH_FILE> <RECREATED_FILE>
```

**Example:**
```bash
./target/release/r_delta patch old_version.bin patch.bin restored.bin
```

### 4\. Verify (The Auditor)

Perform a high-speed, streaming bit-for-bit comparison to prove integrity.

```bash
./target/release/r_delta verify <ORIGINAL_FILE> <RECREATED_FILE>
```

**Example:**
```bash
./target/release/r_delta verify new_version.bin restored.bin
```

### 5\. Sync (The Network Transport)

Synchronize files to a remote server using automatic delta detection and compression.

#### File Sync

The sync command orchestrates the entire pipeline:
1. Connects to the server
2. Server sends its version's signature (if file exists)
3. Client computes delta automatically
4. Client streams optimized patch to server
5. Server reconstructs and verifies

```bash
./target/release/r_delta sync <FILE> <SERVER:PORT>
```

#### Directory Sync

Synchronize entire directory trees with intelligent manifest-based coordination:

1. Client builds lightweight manifest (path, size, modified time, BLAKE3 checksum)
2. Client connects to server and sends manifest via QUIC
3. Server generates sync plan:
   - **SendFull**: New files (upload entire file)
   - **SendDelta**: Modified files (compute and stream delta patch)
   - **Skip**: Identical files (verified by size + content hash)
   - **Delete**: Files on server but not in client (removed after sync completes)
4. Client executes plan:
   - New files uploaded in full
   - Modified files use delta for 50-99% bandwidth savings
   - Identical files skipped entirely
5. Server applies changes and removes deleted files
6. Both sides verify integrity via BLAKE3 checksums

**Example:**
```bash
# Server (runs continuously)
./target/release/r_delta_server

# Client (in another terminal) - Single file
./target/release/r_delta sync file.bin 127.0.0.1:4433

# Client (in another terminal) - Entire directory
./target/release/r_delta sync-dir /path/to/directory 127.0.0.1:4433
```

## 🗺 Roadmap & Architecture

### Workspace Structure (v0.1.2)

The project is organized as a Cargo workspace with clear separation of concerns:

**Design Philosophy:**
- **Core Library (Apache 2.0)**
- **Client CLI (Apache 2.0)**
- **Server (BSL 1.1)**

### Architecture Specs (v0.1.2)

* **Chunking:** FastCDC (Content-Defined Chunking) with Gear Hash.
* **Fingerprinting:** BLAKE3 (SIMD-optimized) for O(1) block identification.
* **Compression:** Hybrid Mode.
  * *Deduplication:* HashMap lookups for known data.
  * *Compression:* Zstd (Streaming Mode) for unknown literals.
* **Transport Protocol:** QUIC (via `quinn`).
  * *Stream 1 (Bi-directional):* Control Plane (Handshakes, Signatures) via Bincode.
  * *Stream 2 (Uni-directional):* Data Plane (Patch Transfer).

### Feature Status

| Feature                 | Status | Description                                    |
|:------------------------|:------:|:-----------------------------------------------|
| **FastCDC Engine**      |   ✅    | Gear Hash + dynamic cut-points                 |
| **Shift Resistance**    |   ✅    | Handles insertions/deletions                   |
| **Hybrid Compression**  |   ✅    | Zstd integration for literal runs              |
| **SKIP Optimization**   |   ✅    | Merges consecutive COPYs (~5% gain)            |
| **QUIC Configuration**  |   ✅    | Server/Client config generators                |
| **File Sync**           |   ✅    | End-to-end network sync (`sync`)               |
| **Directory Sync**      |   ✅    | **Phase 3** Multi-file sync with manifest      |
| **Manifest Generation** |   ✅    | Fast walking with `.gitignore` + checksums     |
| **Diff Algorithm**      |   ✅    | O(n log n) manifest comparison & sync actions  |
| **Content Hashing**     |   ✅    | BLAKE3 checksums for reliable change detection |
| **Sync Planning**       |   ✅    | Protocol for manifest exchange & coordination  |
| **Parallel Transfer**   |  🏗️   | Thread pool for concurrent sync    |

### Why r_delta is Fast

1. **Content-Defined Chunking**: O(1) hash lookups for chunk matching
2. **BLAKE3 Hashing**: SIMD-optimized cryptographic hashing (4-8x faster than SHA-256)
3. **Zstd Compression**: Industry-leading decompression speed (~10x faster than gzip)
4. **SKIP Optimization**: Reduces metadata overhead by ~5% for sequential regions
5. **Streaming Architecture**: Constant memory usage regardless of file size
6. **QUIC Protocol**: Multiplexed streams with 0-RTT connection establishment
7. **Fast Directory Walking**: The `ignore` crate (ripgrep engine) with `.gitignore` awareness for quick manifests
8. **Smart Diff Algorithm**: O(n log n) manifest comparison to identify only changed files
9. **Selective File Transfer**: Skips unchanged files entirely, only syncs SendFull/SendDelta actions

## 📄 Licensing

This repository uses a split licensing model to balance open ecosystem growth with sustainable development:

| Component        | Path            | License        | Usage                                                                                                   |
|:-----------------|:----------------|:---------------|:--------------------------------------------------------------------------------------------------------|
| **Core Library** | `crates/core`   | **Apache 2.0** | Free for any use.                                                                                       |
| **Client CLI**   | `crates/client` | **Apache 2.0** | Free for any use.                                                                                       |
| **Server**       | `crates/server` | **BSL 1.1**    | Free for non-production/personal use. Commercial use requires a license or waiting for the Change Date. |

See [LICENSE](./LICENSE) for the Apache 2.0 terms and [Server License](./crates/server/LICENSE) for the BSL 1.1 terms.