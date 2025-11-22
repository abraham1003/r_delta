use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;
use std::fs::File;
use std::io::{Read, BufReader};
use indicatif::{ProgressBar, ProgressStyle};
use console::style;
use anyhow::Result;
use futures::stream::{self, StreamExt};

async fn handle_sync_dir(directory: PathBuf, server: String) -> Result<(), String> {
    if !directory.exists() {
        return Err(format!("Directory '{}' does not exist", directory.display()));
    }
    if !directory.is_dir() {
        return Err(format!("'{}' is not a directory", directory.display()));
    }

    println!("Synchronizing directory to server...");
    println!("  Directory: '{}'", directory.display());
    println!("  Server: {}", server);

    let start = Instant::now();

    println!("\n→ Scanning directory and building manifest...");
    let manifest = r_delta_core::manifest::Manifest::generate(&directory)
        .map_err(|e| format!("Failed to generate manifest: {}", e))?;

    println!("✓ Found {} items ({} total)", 
        manifest.len(), 
        format_bytes(manifest.total_size() as usize));

    let client_config = r_delta_core::network::generate_client_config()
        .map_err(|e| format!("Failed to create client config: {}", e))?;

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())
        .map_err(|e| format!("Failed to create endpoint: {}", e))?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint.connect(server.parse().unwrap(), "localhost")
        .map_err(|e| format!("Failed to initiate connection: {}", e))?
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    println!("✓ Connected to server");

    let (mut send_stream, mut recv_stream) = connection.open_bi().await
        .map_err(|e| format!("Failed to open control stream: {}", e))?;

    send_message(&mut send_stream, &r_delta_core::protocol::NetMessage::ManifestRequest).await?;

    println!("\n→ Sending manifest to server...");
    const MANIFEST_BATCH_SIZE: usize = 100;
    for chunk in manifest.entries.chunks(MANIFEST_BATCH_SIZE) {
        let packet = r_delta_core::protocol::NetMessage::manifest_packet(chunk.to_vec());
        send_message(&mut send_stream, &packet).await?;
    }
    send_message(&mut send_stream, &r_delta_core::protocol::NetMessage::ManifestEnd).await?;
    println!("✓ Manifest sent");

    println!("\n→ Waiting for sync plan from server...");
    let plan_msg = receive_message(&mut recv_stream).await?;
    
    let sync_plan = match plan_msg {
        r_delta_core::protocol::NetMessage::SyncPlan { items } => {
            r_delta_core::diff::SyncPlan::new(items)
        }
        r_delta_core::protocol::NetMessage::Error { message } => {
            return Err(format!("Server error: {}", message));
        }
        _ => {
            return Err("Expected SyncPlan message".to_string());
        }
    };

    let counts = sync_plan.count_by_action();
    println!("✓ Sync plan received:");
    if let Some(&count) = counts.get("SendFull") {
        println!("  → {} files to upload (new)", count);
    }
    if let Some(&count) = counts.get("SendDelta") {
        println!("  → {} files to sync (modified)", count);
    }
    if let Some(&count) = counts.get("Skip") {
        println!("  → {} files to skip (identical)", count);
    }
    if let Some(&count) = counts.get("Delete") {
        println!("  → {} files to delete", count);
    }

    let files_to_sync = r_delta_core::diff::get_files_to_sync(&sync_plan);
    let files_to_delete: Vec<String> = sync_plan.items.iter()
        .filter(|item| matches!(item.action, r_delta_core::diff::SyncAction::Delete))
        .map(|item| item.path.clone())
        .collect();
    
    if files_to_sync.is_empty() && files_to_delete.is_empty() {
        println!("\n✓ All files are up to date!");
        let duration = start.elapsed();
        println!("  Total time: {}", format_duration(duration));
        return Ok(());
    }

    let files_to_sync_count = files_to_sync.len();
    if !files_to_sync.is_empty() {
        println!("\n→ Syncing {} files...", files_to_sync_count);
        
        let pb = ProgressBar::new(files_to_sync_count as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("#>-"));
            
        let connection = connection.clone();
        let directory = directory.clone();
        let sync_plan_items = sync_plan.items.clone();
        
        let results: Vec<Result<(), String>> = stream::iter(files_to_sync)
            .map(|file_path| {
                let connection = connection.clone();
                let directory = directory.clone();
                let pb = pb.clone();
                let action = sync_plan_items.iter()
                    .find(|item| &item.path == &file_path)
                    .map(|item| item.action.clone());
                let file_path = file_path.clone();
                    
                async move {
                    let full_path = directory.join(&file_path);
                    pb.set_message(format!("Syncing {}", file_path));
                    
                    let result = match action {
                        Some(r_delta_core::diff::SyncAction::SendFull) => {
                            sync_single_file(&full_path, &file_path, &connection, false, true).await
                        }
                        Some(r_delta_core::diff::SyncAction::SendDelta) => {
                            sync_single_file(&full_path, &file_path, &connection, true, true).await
                        }
                        _ => Ok(()),
                    };
                    
                    pb.inc(1);
                    result
                }
            })
            .buffer_unordered(50)
            .collect()
            .await;
            
        pb.finish_with_message("Sync complete");
        
        let errors: Vec<String> = results.into_iter()
            .filter_map(|r| r.err())
            .collect();
            
        if !errors.is_empty() {
            println!("\n⚠️ Encountered {} errors during sync:", errors.len());
            for err in errors.iter().take(5) {
                println!("  - {}", err);
            }
            if errors.len() > 5 {
                println!("  ... and {} more", errors.len() - 5);
            }
        }
    }

    if !files_to_delete.is_empty() {
        println!("\n→ Server will delete {} obsolete files", files_to_delete.len());
        for file_path in &files_to_delete {
            println!("  ✗ {}", file_path);
        }
    }

    let duration = start.elapsed();
    println!("\n✓ Directory synchronization complete!");
    println!("  Total time: {}", format_duration(duration));
    println!("  Files processed: {}", files_to_sync_count);

    connection.close(0u32.into(), b"done");
    endpoint.wait_idle().await;

    Ok(())
}

async fn sync_single_file(
    file_path: &PathBuf,
    relative_path: &str,
    connection: &quinn::Connection,
    use_delta: bool,
    silent: bool,
) -> Result<(), String> {
    let metadata = std::fs::metadata(file_path)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;
    let file_size = metadata.len();

    const SMALL_FILE_THRESHOLD: u64 = 100 * 1024;

    if file_size < SMALL_FILE_THRESHOLD {
        if !silent { 
            println!("  → Small file detected, using compressed upload"); 
        }
        
        let (mut send_stream, mut recv_stream) = connection.open_bi().await
            .map_err(|e| format!("Failed to open stream: {}", e))?;

        let compressed_msg = r_delta_core::protocol::NetMessage::SendFullCompressed {
            filename: relative_path.to_string(),
            original_size: file_size,
        };
        send_message(&mut send_stream, &compressed_msg).await?;

        handle_compressed_upload(file_path, connection).await?;

        let verify_msg = receive_message(&mut recv_stream).await?;
        match verify_msg {
            r_delta_core::protocol::NetMessage::VerifyResult { matches, .. } => {
                if matches {
                    if !silent { println!("  ✓ Verified"); }
                } else {
                    return Err("Server verification failed".to_string());
                }
            }
            _ => {
                return Err("Expected verification result".to_string());
            }
        }

        return Ok(());
    }

    let (mut send_stream, mut recv_stream) = connection.open_bi().await
        .map_err(|e| format!("Failed to open stream: {}", e))?;

    let handshake = r_delta_core::protocol::NetMessage::handshake(
        relative_path.to_string(), 
        file_size
    );
    send_message(&mut send_stream, &handshake).await?;

    let ack = receive_message(&mut recv_stream).await?;

    match ack {
        r_delta_core::protocol::NetMessage::HandshakeAck { has_old_file } => {
            if has_old_file && use_delta {
                if !silent { println!("  → Delta sync"); }
                handle_differential_sync(
                    file_path,
                    connection,
                    &mut send_stream,
                    &mut recv_stream,
                ).await?;
            } else {
                if !silent { println!("  → Full upload"); }
                handle_full_upload(file_path, connection).await?;
            }
        }
        r_delta_core::protocol::NetMessage::Error { message } => {
            return Err(format!("Server error: {}", message));
        }
        _ => {
            return Err("Unexpected response from server".to_string());
        }
    }

    let verify_msg = receive_message(&mut recv_stream).await?;
    match verify_msg {
        r_delta_core::protocol::NetMessage::VerifyResult { matches, .. } => {
            if matches {
                if !silent { println!("  ✓ Verified"); }
            } else {
                return Err("Server verification failed".to_string());
            }
        }
        _ => {
            return Err("Expected verification result".to_string());
        }
    }

    Ok(())
}

async fn handle_sync(file: PathBuf, server: String) -> Result<(), String> {
    validate_file_exists(&file, "File")?;

    let filename = file.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid filename".to_string())?
        .to_string();

    let metadata = std::fs::metadata(&file)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;
    let file_size = metadata.len();

    println!("Synchronizing file to server...");
    println!("  File: '{}'", file.display());
    println!("  Size: {}", format_bytes(file_size as usize));
    println!("  Server: {}", server);

    let start = Instant::now();

    let client_config = r_delta_core::network::generate_client_config()
        .map_err(|e| format!("Failed to create client config: {}", e))?;

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())
        .map_err(|e| format!("Failed to create endpoint: {}", e))?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint.connect(server.parse().unwrap(), "localhost")
        .map_err(|e| format!("Failed to initiate connection: {}", e))?
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    println!("\n✓ Connected to server");

    let (mut send_stream, mut recv_stream) = connection.open_bi().await
        .map_err(|e| format!("Failed to open control stream: {}", e))?;

    let handshake = r_delta_core::protocol::NetMessage::handshake(filename.clone(), file_size);
    send_message(&mut send_stream, &handshake).await?;

    let ack = receive_message(&mut recv_stream).await?;

    match ack {
        r_delta_core::protocol::NetMessage::HandshakeAck { has_old_file } => {
            println!("✓ Handshake complete");
            println!("  Server has old version: {}", if has_old_file { "yes" } else { "no" });

            if has_old_file {
                handle_differential_sync(
                    &file,
                    &connection,
                    &mut send_stream,
                    &mut recv_stream,
                ).await?;
            } else {
                handle_full_upload(&file, &connection).await?;
            }
        }
        r_delta_core::protocol::NetMessage::Error { message } => {
            return Err(format!("Server error: {}", message));
        }
        _ => {
            return Err("Unexpected response from server".to_string());
        }
    }

    let verify_msg = receive_message(&mut recv_stream).await?;
    match verify_msg {
        r_delta_core::protocol::NetMessage::VerifyResult { matches, checksum } => {
            if matches {
                let duration = start.elapsed();
                println!("\n✓ Synchronization complete!");
                println!("  Checksum: {}", hex::encode(checksum));
                println!("  Total time: {}", format_duration(duration));
            } else {
                return Err("Server verification failed".to_string());
            }
        }
        _ => {
            return Err("Expected verification result".to_string());
        }
    }

    connection.close(0u32.into(), b"done");
    endpoint.wait_idle().await;

    Ok(())
}

async fn handle_differential_sync(
    file: &PathBuf,
    connection: &quinn::Connection,
    _send_stream: &mut quinn::SendStream,
    recv_stream: &mut quinn::RecvStream,
) -> Result<(), String> {
    println!("\n→ Receiving signatures from server...");

    let request_sig = receive_message(recv_stream).await?;
    if !matches!(request_sig, r_delta_core::protocol::NetMessage::RequestSignature) {
        return Err("Expected RequestSignature message".to_string());
    }

    let mut all_signatures = Vec::new();
    loop {
        let msg = receive_message(recv_stream).await?;
        match msg {
            r_delta_core::protocol::NetMessage::SignaturePacket { signatures } => {
                all_signatures.extend(signatures.into_iter().map(|s| s.into()));
            }
            r_delta_core::protocol::NetMessage::SignatureEnd => {
                break;
            }
            _ => {
                return Err("Unexpected message while receiving signatures".to_string());
            }
        }
    }

    println!("✓ Received {} signatures", all_signatures.len());

    println!("\n→ Computing delta...");
    
    let file_size = std::fs::metadata(file)
        .map_err(|e| format!("Failed to get file size: {}", e))?
        .len() as usize;
    
    let generator = r_delta_core::delta::DeltaGenerator::new(all_signatures);
    let instructions = generate_delta_instructions(&generator, file)?;

    let mut patch_size: usize = 0;
    let mut bytes_matched: usize = 0;
    
    for i in &instructions {
        match i {
            r_delta_core::delta::PatchInstruction::Copy(_, len) => {
                patch_size += 17;
                bytes_matched += *len;
            },
            r_delta_core::delta::PatchInstruction::Literal(d) => {
                patch_size += 9 + d.len();
            },
            r_delta_core::delta::PatchInstruction::CompressedLiteral { compressed_data, .. } => {
                patch_size += 13 + compressed_data.len();
            },
        }
    }

    let dedup_ratio = if file_size > 0 {
        (bytes_matched as f64 / file_size as f64) * 100.0
    } else {
        0.0
    };

    println!("✓ Delta computed");
    println!("  Instructions: {}", instructions.len());
    println!("  Patch size: {}", format_bytes(patch_size));

    println!("\n→ Streaming delta to server...");

    let pb = ProgressBar::new(patch_size as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("█▓░")
    );

    let mut data_stream = connection.open_uni().await
        .map_err(|e| format!("Failed to open data stream: {}", e))?;

    let start_patch = r_delta_core::protocol::NetMessage::StartPatch;
    send_message_on_stream(&mut data_stream, &start_patch).await?;

    let mut bytes_sent = 0;
    for instruction in &instructions {
        let bytes = instruction.to_bytes();
        data_stream.write_all(&bytes).await
            .map_err(|e| format!("Failed to write instruction: {}", e))?;
        bytes_sent += bytes.len();
        pb.set_position(bytes_sent as u64);
    }

    data_stream.finish()
        .map_err(|e| format!("Failed to close data stream: {}", e))?;

    pb.finish_and_clear();
    println!("✓ Delta transmitted");
    
    println!("\n{}", style("═══ Deduplication Report ═══").bold().cyan());
    println!("  Original Size:  {}", style(format_bytes(file_size)).yellow());
    println!("  Transferred:    {}", style(format_bytes(patch_size)).green());
    
    let savings = if file_size > 0 {
        100.0 - ((patch_size as f64 / file_size as f64) * 100.0)
    } else {
        0.0
    };
    
    println!("  Savings:        {}", style(format!("{:.2}%", savings)).bold().green());
    println!("  Match Rate:     {}", style(format!("{:.2}%", dedup_ratio)).cyan());

    Ok(())
}

async fn handle_full_upload(
    file: &PathBuf,
    connection: &quinn::Connection,
) -> Result<(), String> {
    let file_size = std::fs::metadata(file)
        .map_err(|e| format!("Failed to get file size: {}", e))?
        .len();

    println!("\n→ Uploading full file...");

    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("█▓░")
    );

    let mut data_stream = connection.open_uni().await
        .map_err(|e| format!("Failed to open data stream: {}", e))?;

    let mut file_reader = BufReader::new(File::open(file)
        .map_err(|e| format!("Failed to open file: {}", e))?);

    let mut buffer = vec![0u8; 64 * 1024];
    let mut total_sent = 0u64;

    loop {
        let n = file_reader.read(&mut buffer)
            .map_err(|e| format!("Failed to read file: {}", e))?;
        if n == 0 {
            break;
        }

        data_stream.write_all(&buffer[..n]).await
            .map_err(|e| format!("Failed to write to stream: {}", e))?;
        total_sent += n as u64;
        pb.set_position(total_sent);
    }

    data_stream.finish()
        .map_err(|e| format!("Failed to close data stream: {}", e))?;

    pb.finish_and_clear();
    println!("✓ File uploaded: {}", format_bytes(total_sent as usize));

    Ok(())
}

async fn handle_compressed_upload(
    file: &PathBuf,
    connection: &quinn::Connection,
) -> Result<(), String> {
    let file_size = std::fs::metadata(file)
        .map_err(|e| format!("Failed to get file size: {}", e))?
        .len();

    println!("\n→ Uploading compressed file...");

    let mut file_data = Vec::new();
    let mut file_reader = File::open(file)
        .map_err(|e| format!("Failed to open file: {}", e))?;
    file_reader.read_to_end(&mut file_data)
        .map_err(|e| format!("Failed to read file: {}", e))?;

    let compressed_data = zstd::encode_all(file_data.as_slice(), 3)
        .map_err(|e| format!("Failed to compress file: {}", e))?;

    let pb = ProgressBar::new(compressed_data.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("█▓░")
    );

    let mut data_stream = connection.open_uni().await
        .map_err(|e| format!("Failed to open data stream: {}", e))?;

    data_stream.write_all(&compressed_data).await
        .map_err(|e| format!("Failed to write to stream: {}", e))?;
    pb.set_position(compressed_data.len() as u64);

    data_stream.finish()
        .map_err(|e| format!("Failed to close data stream: {}", e))?;

    pb.finish_and_clear();
    
    let compression_ratio = (compressed_data.len() as f64 / file_size as f64) * 100.0;
    println!("✓ File compressed and uploaded: {} → {} ({:.1}%)", 
             format_bytes(file_size as usize),
             format_bytes(compressed_data.len()),
             compression_ratio);

    Ok(())
}

fn generate_delta_instructions(
    generator: &r_delta_core::delta::DeltaGenerator,
    new_file_path: &PathBuf,
) -> Result<Vec<r_delta_core::delta::PatchInstruction>, String> {
    const MAX_LITERAL_BUFFER: usize = 8 * 1024 * 1024;
    const COMPRESSION_THRESHOLD: usize = 64;

    let new_file = File::open(new_file_path)
        .map_err(|e| format!("Failed to open file: {}", e))?;
    let file_size = new_file.metadata()
        .map_err(|e| format!("Failed to get metadata: {}", e))?
        .len() as usize;

    if file_size == 0 {
        return Ok(Vec::new());
    }

    let mut chunker = r_delta_core::chunker::Chunker::new(new_file);
    let mut instructions = Vec::new();
    let mut literal_buffer = Vec::new();

    let flush_literal_buffer = |buffer: &mut Vec<u8>, instructions: &mut Vec<r_delta_core::delta::PatchInstruction>| {
        if buffer.is_empty() {
            return;
        }

        let literal_len = buffer.len();

        if literal_len < COMPRESSION_THRESHOLD {
            instructions.push(r_delta_core::delta::PatchInstruction::Literal(buffer.clone()));
        } else {
            match zstd::encode_all(buffer.as_slice(), 3) {
                Ok(compressed_data) => {
                    if compressed_data.len() < literal_len {
                        instructions.push(r_delta_core::delta::PatchInstruction::CompressedLiteral {
                            decompressed_len: literal_len as u64,
                            compressed_data,
                        });
                    } else {
                        instructions.push(r_delta_core::delta::PatchInstruction::Literal(buffer.clone()));
                    }
                }
                Err(_) => {
                    instructions.push(r_delta_core::delta::PatchInstruction::Literal(buffer.clone()));
                }
            }
        }

        buffer.clear();
    };

    while let Some(chunk) = chunker.next_chunk()
        .map_err(|e| format!("Chunking failed: {}", e))? {

        if let Some((offset, length)) = generator.find_match(&chunk.data) {
            flush_literal_buffer(&mut literal_buffer, &mut instructions);
            instructions.push(r_delta_core::delta::PatchInstruction::Copy(offset, length));
        } else {
            literal_buffer.extend_from_slice(&chunk.data);

            if literal_buffer.len() >= MAX_LITERAL_BUFFER {
                flush_literal_buffer(&mut literal_buffer, &mut instructions);
            }
        }
    }

    flush_literal_buffer(&mut literal_buffer, &mut instructions);

    Ok(instructions)
}

async fn send_message(stream: &mut quinn::SendStream, msg: &r_delta_core::protocol::NetMessage) -> Result<(), String> {
    let data = msg.serialize()
        .map_err(|e| format!("Serialization failed: {}", e))?;
    let len = data.len() as u32;

    stream.write_all(&len.to_le_bytes()).await
        .map_err(|e| format!("Failed to write length: {}", e))?;
    stream.write_all(&data).await
        .map_err(|e| format!("Failed to write message: {}", e))?;

    Ok(())
}

async fn send_message_on_stream(stream: &mut quinn::SendStream, msg: &r_delta_core::protocol::NetMessage) -> Result<(), String> {
    let data = msg.serialize()
        .map_err(|e| format!("Serialization failed: {}", e))?;
    let len = data.len() as u32;

    stream.write_all(&len.to_le_bytes()).await
        .map_err(|e| format!("Failed to write length: {}", e))?;
    stream.write_all(&data).await
        .map_err(|e| format!("Failed to write message: {}", e))?;

    Ok(())
}

async fn receive_message(stream: &mut quinn::RecvStream) -> Result<r_delta_core::protocol::NetMessage, String> {
    use quinn::ReadExactError;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await
        .map_err(|e| match e {
            ReadExactError::FinishedEarly(_) => "Connection closed".to_string(),
            ReadExactError::ReadError(e) => format!("Read error: {}", e),
        })?;
    let msg_len = u32::from_le_bytes(len_buf) as usize;

    let mut msg_buf = vec![0u8; msg_len];
    stream.read_exact(&mut msg_buf).await
        .map_err(|e| match e {
            ReadExactError::FinishedEarly(_) => "Connection closed while reading message".to_string(),
            ReadExactError::ReadError(e) => format!("Read error: {}", e),
        })?;

    r_delta_core::protocol::NetMessage::deserialize(&msg_buf)
        .map_err(|e| format!("Deserialization failed: {}", e))
}


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
    #[command(about = "Sync file to remote server")]
    Sync {
        #[arg(value_name = "FILE", help = "File to synchronize")]
        file: PathBuf,

        #[arg(value_name = "SERVER", help = "Server address (IP:PORT)")]
        server: String,
    },

    #[command(about = "Sync directory to remote server")]
    SyncDir {
        #[arg(value_name = "DIRECTORY", help = "Directory to synchronize")]
        directory: PathBuf,

        #[arg(value_name = "SERVER", help = "Server address (IP:PORT)")]
        server: String,
    },

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

fn validate_file_exists(path: &Path, description: &str) -> Result<(), String> {
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

fn validate_output_path(path: &Path) -> Result<(), String> {
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

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
    );
    spinner.set_message(format!("Generating signature for '{}'...", input_file.display()));
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let start = Instant::now();

    let signatures = r_delta_core::signature::generate_signature(&input_file, &output_sig)
        .map_err(|e| format!("Failed to generate signature: {}", e))?;

    let duration = start.elapsed();
    spinner.finish_and_clear();

    let input_size = std::fs::metadata(&input_file)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    let sig_size = std::fs::metadata(&output_sig)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    println!("✓ Signature created in {}", format_duration(duration));
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

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
    );
    spinner.set_message("Computing delta...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let start = Instant::now();

    let signatures = r_delta_core::signature::read_signature_file(&signature_file)
        .map_err(|e| format!("Failed to read signature file: {}", e))?;

    let generator = r_delta_core::delta::DeltaGenerator::new(signatures);
    let stats = generator
        .generate_delta(&new_file, &output_patch)
        .map_err(|e| format!("Failed to generate delta: {}", e))?;

    let duration = start.elapsed();
    spinner.finish_and_clear();

    let reduction = if stats.new_file_size > 0 {
        100.0 - stats.compression_ratio()
    } else {
        0.0
    };

    println!("✓ Delta created in {}", format_duration(duration));
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

    let stats = r_delta_core::patch::apply_patch(&old_file, &patch_file, &output_file)
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

    let result = r_delta_core::verify::verify_files(&file_a, &file_b)
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
        Commands::Sync { file, server } => {
            match tokio::runtime::Runtime::new() {
                Ok(rt) => rt.block_on(handle_sync(file, server)),
                Err(e) => Err(format!("Failed to create async runtime: {}", e)),
            }
        }

        Commands::SyncDir { directory, server } => {
            match tokio::runtime::Runtime::new() {
                Ok(rt) => rt.block_on(handle_sync_dir(directory, server)),
                Err(e) => Err(format!("Failed to create async runtime: {}", e)),
            }
        }

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
