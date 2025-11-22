# This script requires rclone (https://rclone.org/) to be present in the same directory.
import os
import time
import subprocess
import random
import shutil
from pathlib import Path

R_DELTA_EXE = r"target\release\r_delta.exe"
SERVER_EXE = r"target\release\r_delta_server.exe"
RCLONE_EXE = r"rclone.exe"
SERVER_ADDR = "127.0.0.1:4433"

DATA_DIR = Path("bench_data")
SRC_DIR = DATA_DIR / "src"
DST_RCLONE = DATA_DIR / "dst_rclone"
DST_RDELTA = DATA_DIR / "dst_r_delta"
def generate_data():
    if DATA_DIR.exists():
        shutil.rmtree(DATA_DIR)
    SRC_DIR.mkdir(parents=True)
    DST_RCLONE.mkdir(parents=True)
    DST_RDELTA.mkdir(parents=True)

    print("Generating initial dataset...")
    print("Creating 500 tiny files (1KB each)")
    for i in range(500):
        with open(SRC_DIR / f"tiny_{i}.txt", "wb") as f:
            f.write(os.urandom(1024))

    print("Creating 10 medium files (1MB each)")
    for i in range(10):
        with open(SRC_DIR / f"med_{i}.bin", "wb") as f:
            f.write(b"A" * 500_000 + os.urandom(500_000))

    print("Creating 1 large file (50MB)")
    with open(SRC_DIR / "large.iso", "wb") as f:
        f.write(os.urandom(1024 * 1024 * 50))
    
    print(f"Dataset created: {calculate_size(SRC_DIR)}")

def mutate_data():
    print("\nApplying modifications to dataset...")
    print("Modifying 50 tiny files")
    for i in range(0, 500, 10):
        with open(SRC_DIR / f"tiny_{i}.txt", "ab") as f:
            f.write(b"APPENDED_DATA")

    print("Inserting 1KB into middle of large.iso")
    path = SRC_DIR / "large.iso"
    with open(path, "rb") as f:
        data = f.read()
    new_data = data[:1024*1024] + os.urandom(1024) + data[1024*1024:]
    with open(path, "wb") as f:
        f.write(new_data)
    
    print(f"Modifications applied: {calculate_size(SRC_DIR)}")

def calculate_size(path):
    total = 0
    for entry in path.rglob('*'):
        if entry.is_file():
            total += entry.stat().st_size
    
    if total >= 1024 * 1024:
        return f"{total / (1024 * 1024):.2f} MB"
    elif total >= 1024:
        return f"{total / 1024:.2f} KB"
    else:
        return f"{total} bytes"

def run_rclone():
    print("\nRunning rclone sync...")
    start = time.time()
    cmd = [RCLONE_EXE, "sync", str(SRC_DIR), str(DST_RCLONE), "--stats", "0"]
    result = subprocess.run(cmd, capture_output=True, text=False)
    duration = time.time() - start
    
    if result.returncode != 0:
        print("Rclone failed:", result.stderr.decode('utf-8', errors='ignore'))
        return None
    
    print(f"Rclone completed: {duration:.4f}s")
    return duration

def run_r_delta():
    print("\nRunning r_delta sync...")
    server_storage = Path("server_storage")
    if server_storage.exists():
        shutil.rmtree(server_storage)
    
    print("Starting r_delta_server...")
    server_proc = subprocess.Popen(
        [SERVER_EXE],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL
    )
    time.sleep(2)
    
    start = time.time()
    cmd = [R_DELTA_EXE, "sync-dir", str(SRC_DIR), SERVER_ADDR]
    result = subprocess.run(cmd, capture_output=True, text=False)
    duration = time.time() - start
    
    server_proc.terminate()
    try:
        server_proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        server_proc.kill()
    
    if result.returncode != 0:
        stderr_text = result.stderr.decode('utf-8', errors='ignore') if result.stderr else "Unknown error"
        print("r_delta failed:", stderr_text)
        return None
    
    print(f"r_delta completed: {duration:.4f}s")
    
    if result.stdout:
        try:
            output = result.stdout.decode('utf-8', errors='ignore')
            if "Files processed:" in output:
                for line in output.split('\n'):
                    if "Files processed:" in line or "Total time:" in line:
                        print(f"{line.strip()}")
        except:
            pass
    
    return duration

def print_comparison(label, t_rclone, t_rdelta):
    print(f"\n{label}")
    print(f"Rclone:  {t_rclone:.4f}s")
    print(f"r_delta: {t_rdelta:.4f}s")
    
    if t_rclone and t_rdelta:
        if t_rdelta < t_rclone:
            speedup = ((t_rclone - t_rdelta) / t_rclone) * 100
            print(f"r_delta faster by {speedup:.1f}%")
        elif t_rclone < t_rdelta:
            slowdown = ((t_rdelta - t_rclone) / t_rclone) * 100
            print(f"r_delta slower by {slowdown:.1f}%")
        else:
            print("Same performance")
if __name__ == "__main__":
    print("r_delta vs rclone benchmark\n")
    
    if not Path(R_DELTA_EXE).exists():
        print(f"Error: {R_DELTA_EXE} not found")
        print("Run: cargo build --release")
        exit(1)
    
    if not Path(SERVER_EXE).exists():
        print(f"Error: {SERVER_EXE} not found")
        print("Run: cargo build --release")
        exit(1)
    
    if not Path(RCLONE_EXE).exists():
        print(f"Error: {RCLONE_EXE} not found")
        print("Download from: https://rclone.org/downloads/")
        exit(1)
    
    print("All executables found\n")
    
    print("ROUND 1: Fresh sync")
    print("-" * 50)
    generate_data()
    t_rclone_1 = run_rclone()
    t_rdelta_1 = run_r_delta()
    
    if t_rclone_1 and t_rdelta_1:
        print_comparison("Round 1 results:", t_rclone_1, t_rdelta_1)
    
    print("\n\nROUND 2: Incremental sync")
    print("-" * 50)
    mutate_data()
    t_rclone_2 = run_rclone()
    t_rdelta_2 = run_r_delta()
    
    if t_rclone_2 and t_rdelta_2:
        print_comparison("Round 2 results:", t_rclone_2, t_rdelta_2)
    
    print("\n\nFinal results:")
    print("-" * 50)
    
    if all([t_rclone_1, t_rdelta_1, t_rclone_2, t_rdelta_2]):
        print(f"Fresh sync:")
        print(f"  Rclone:  {t_rclone_1:.4f}s")
        print(f"  r_delta: {t_rdelta_1:.4f}s")
        print(f"\nIncremental sync:")
        print(f"  Rclone:  {t_rclone_2:.4f}s")
        print(f"  r_delta: {t_rdelta_2:.4f}s")
        
        if t_rdelta_2 < t_rclone_2:
            speedup = ((t_rclone_2 - t_rdelta_2) / t_rclone_2) * 100
            print(f"\nIncremental sync: r_delta is {speedup:.1f}% faster")
    else:
        print("Some tests failed")
    
    print(f"\nBenchmark complete")
    print(f"Data saved in: {DATA_DIR.absolute()}")
