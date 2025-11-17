# r_delta

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-Pre--Alpha-red.svg)]()

**r_delta** is a high-performance, memory-safe binary delta compression tool. It allows you to synchronize large files between systems by transferring *only* the modified bytes, not the entire file.

> **⚠️ Current Status:** This project is in active early development. It currently uses **Fixed-Size Chunking**. We are actively transitioning to **Content-Defined Chunking (FastCDC)** for the v0.2.0 release to support shift-resistant deduplication.

## ⚡ Why r_delta?

Most delta-transfer tools are either legacy C implementations (hard to maintain, memory unsafe) or tied to specific ecosystems. `r_delta` aims to be the modern standard:

* **Memory Safe:** Written in pure Rust.
* **Zero Runtime Dependencies:** Produces a single, static binary.
* **Algorithmically Transparent:** We clearly document our hashing and chunking strategies (see Roadmap).

## 🛠 Installation

`r_delta` is not yet published to crates.io. To use it, build from source:

```bash
git clone [https://github.com/abraham1003/r_delta](https://github.com/abraham1003/r_delta)
cd r_delta
cargo build --release
````

The binary will be located at `./target/release/r_delta`.

## 🚀 Usage

`r_delta` operates in three distinct phases: **Signature**, **Delta**, and **Patch**.

### 1\. Generate a Signature

Create a lightweight "map" of the original file.

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

We believe in building in public. Our goal is to surpass `rsync` efficiency by implementing state-of-the-art chunking algorithms.

| Feature | Status | Description |
| :--- | :---: | :--- |
| **Core Logic** | ✅ | Basic signature/delta/patch workflow |
| **Fixed Chunking** | ✅ | Current implementation (4KB blocks). Vulnerable to bit-shifts. |
| **Strong Hashing** | ✅ | SHA-256 verification for data integrity. |
| **FastCDC / Gear Hash** | 🚧 | **In Progress.** Implementing Content-Defined Chunking to handle data insertions/deletions without losing synchronization. |
| **Compression** | ⬜ | Apply Zstd/Lz4 to the literal data inside the patch file. |
| **Network Layer** | ⬜ | Native `r_delta` protocol over TCP/UDP (Future "Engine"). |

## 🤝 Contributing

We are currently rewriting the core `chunker` module to support variable-size blocks. If you are interested in **FastCDC**, **Rolling Hashes**, or **SIMD optimization**, check the Issues tab.

## 📄 License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.