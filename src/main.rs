use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "r_delta")]
#[command(author = "Abraham Thomas")]
#[command(version = "0.1.2")]
#[command(about = "Delta synchronization tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Generate signature file from source file")]
    Signature {
        #[arg(value_name = "INPUT_FILE", help = "Source file to create signature from")]
        input_file: PathBuf,

        #[arg(value_name = "OUTPUT_SIG", help = "Output signature file path")]
        output_sig: PathBuf,
    },

    #[command(about = "Generate delta patch from signature and new file")]
    Delta {
        #[arg(value_name = "SIGNATURE_FILE", help = "Signature file from old version")]
        signature_file: PathBuf,

        #[arg(value_name = "NEW_FILE", help = "New version of the file")]
        new_file: PathBuf,

        #[arg(value_name = "OUTPUT_PATCH", help = "Output patch file path")]
        output_patch: PathBuf,
    },

    #[command(about = "Apply patch to reconstruct new file")]
    Patch {
        #[arg(value_name = "OLD_FILE", help = "Original old file")]
        old_file: PathBuf,

        #[arg(value_name = "PATCH_FILE", help = "Patch file to apply")]
        patch_file: PathBuf,

        #[arg(value_name = "OUTPUT_FILE", help = "Output reconstructed file path")]
        output_file: PathBuf,
    },

    #[command(about = "Verify two files are identical")]
    Verify {
        #[arg(value_name = "FILE_A", help = "First file to compare")]
        file_a: PathBuf,

        #[arg(value_name = "FILE_B", help = "Second file to compare")]
        file_b: PathBuf,
    },
}

fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    const GB: usize = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.2}s", secs)
    } else {
        format!("{:.0}ms", secs * 1000.0)
    }
}

fn validate_file_exists(path: &PathBuf, description: &str) -> Result<(), String> {
    if !path.exists() {
        return Err(format!(
            "Error: {} '{}' does not exist.",
            description,
            path.display()
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "Error: {} '{}' is not a file.",
            description,
            path.display()
        ));
    }
    Ok(())
}

fn validate_output_path(path: &PathBuf) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!(
                "Error: Output directory '{}' does not exist.",
                parent.display()
            ));
        }
    }
    Ok(())
}

fn handle_signature(input_file: PathBuf, output_sig: PathBuf) -> Result<(), String> {
    validate_file_exists(&input_file, "Input file")?;
    validate_output_path(&output_sig)?;

    println!("Generating signature for '{}'...", input_file.display());
    let start = Instant::now();

    let signatures = r_delta::signature::generate_signature(&input_file, &output_sig)
        .map_err(|e| format!("Failed to generate signature: {}", e))?;

    let duration = start.elapsed();

    let input_size = std::fs::metadata(&input_file)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    let sig_size = std::fs::metadata(&output_sig)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    println!("\n✓ Signature created in {}", format_duration(duration));
    println!("  Source file: {}", format_bytes(input_size));
    println!("  Signature size: {}", format_bytes(sig_size));
    println!("  Chunks: {}", signatures.len());
    
    if !signatures.is_empty() {
        let avg_chunk = input_size / signatures.len();
        println!("  Avg chunk size: {} bytes", avg_chunk);
    }

    Ok(())
}

fn handle_delta(
    signature_file: PathBuf,
    new_file: PathBuf,
    output_patch: PathBuf,
) -> Result<(), String> {
    validate_file_exists(&signature_file, "Signature file")?;
    validate_file_exists(&new_file, "New file")?;
    validate_output_path(&output_patch)?;

    println!("Calculating delta...");
    println!("  Signature: '{}'", signature_file.display());
    println!("  New file: '{}'", new_file.display());

    let start = Instant::now();

    let signatures = r_delta::signature::read_signature_file(&signature_file)
        .map_err(|e| format!("Failed to read signature file: {}", e))?;

    let generator = r_delta::delta::DeltaGenerator::new(signatures);
    let stats = generator
        .generate_delta(&new_file, &output_patch)
        .map_err(|e| format!("Failed to generate delta: {}", e))?;

    let duration = start.elapsed();

    let reduction = if stats.new_file_size > 0 {
        100.0 - stats.compression_ratio()
    } else {
        0.0
    };

    println!("\n✓ Delta created in {}", format_duration(duration));
    println!("  Original size: {}", format_bytes(stats.new_file_size));
    println!("  Patch size: {}", format_bytes(stats.patch_file_size));
    println!("  Reduction: {:.2}%", reduction);
    println!("\n  Match efficiency: {:.2}%", stats.match_percentage());
    println!("  Copy instructions: {}", stats.copy_instructions);
    println!("  Literal instructions: {}", stats.literal_instructions);

    Ok(())
}

fn handle_patch(
    old_file: PathBuf,
    patch_file: PathBuf,
    output_file: PathBuf,
) -> Result<(), String> {
    validate_file_exists(&old_file, "Old file")?;
    validate_file_exists(&patch_file, "Patch file")?;
    validate_output_path(&output_file)?;

    println!("Reconstructing file...");
    println!("  Old file: '{}'", old_file.display());
    println!("  Patch: '{}'", patch_file.display());

    let start = Instant::now();

    let stats = r_delta::patch::apply_patch(&old_file, &patch_file, &output_file)
        .map_err(|e| format!("Failed to apply patch: {}", e))?;

    let duration = start.elapsed();

    let output_size = std::fs::metadata(&output_file)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    println!("\n✓ File successfully reconstructed in {}", format_duration(duration));
    println!("  Output size: {}", format_bytes(output_size));
    println!("  Bytes written: {}", format_bytes(stats.bytes_written));
    println!("  Copy operations: {}", stats.copy_instructions);
    println!("  Literal operations: {}", stats.literal_instructions);

    let copy_percent = if stats.bytes_written > 0 {
        (stats.bytes_copied as f64 / stats.bytes_written as f64) * 100.0
    } else {
        0.0
    };
    println!("  Reused from old: {:.2}%", copy_percent);

    Ok(())
}

fn handle_verify(file_a: PathBuf, file_b: PathBuf) -> Result<(), String> {
    validate_file_exists(&file_a, "First file")?;
    validate_file_exists(&file_b, "Second file")?;

    println!("Verifying files...");
    println!("  File A: '{}'", file_a.display());
    println!("  File B: '{}'", file_b.display());

    let start = Instant::now();

    let result = r_delta::verify::verify_files(&file_a, &file_b)
        .map_err(|e| format!("Verification failed: {}", e))?;

    let duration = start.elapsed();

    if result.are_equal {
        println!("\n✓ Files are identical!");
        println!("  Size: {}", format_bytes(result.file_a_size as usize));
        println!("  Verification time: {}", format_duration(duration));

        let throughput = if duration.as_secs_f64() > 0.0 {
            result.file_a_size as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        println!("  Throughput: {}/s", format_bytes(throughput as usize));
    } else {
        println!("\n✗ Files are different!");
        println!("  File A size: {}", format_bytes(result.file_a_size as usize));
        println!("  File B size: {}", format_bytes(result.file_b_size as usize));

        if let Some(offset) = result.first_mismatch_offset {
            println!("  First mismatch at offset: {} (0x{:X})", offset, offset);
        } else if result.file_a_size != result.file_b_size {
            println!("  Size mismatch detected");
        }
        println!("  Verification time: {}", format_duration(duration));
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Signature {
            input_file,
            output_sig,
        } => handle_signature(input_file, output_sig),

        Commands::Delta {
            signature_file,
            new_file,
            output_patch,
        } => handle_delta(signature_file, new_file, output_patch),

        Commands::Patch {
            old_file,
            patch_file,
            output_file,
        } => handle_patch(old_file, patch_file, output_file),

        Commands::Verify {
            file_a,
            file_b,
        } => handle_verify(file_a, file_b),
    };

    if let Err(e) = result {
        eprintln!("\n{}", e);
        process::exit(1);
    }
}