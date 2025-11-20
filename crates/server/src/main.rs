use std::fs::{File, create_dir_all, rename};
use std::io::{self, Read, Write, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{info, warn, error};
use r_delta_core::{
    protocol::NetMessage,
    network::generate_server_config,
    signature::ChunkSignature,
    chunker::Chunker,
    patch::StreamingPatchBuilder,
    delta::PatchInstruction,
};

const SIGNATURE_BATCH_SIZE: usize = 100;
const BUFFER_SIZE: usize = 64 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

    info!("R-Delta Server v0.1.2");

    let config = generate_server_config()?;
    let endpoint = quinn::Endpoint::server(config, "0.0.0.0:4433".parse()?)?;

    info!("Listening on: 0.0.0.0:4433");

    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            if let Err(e) = handle_connection(incoming).await {
                error!("Connection error: {}", e);
            }
        });
    }

    Ok(())
}

async fn handle_connection(incoming: quinn::Incoming) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();
    let connection = incoming.await?;
    let remote_addr = connection.remote_address();
    info!("New connection from {}", remote_addr);

    let (mut send_stream, mut recv_stream) = connection.accept_bi().await?;

    let handshake_msg = receive_message(&mut recv_stream).await?;

    let (filename, file_size) = match handshake_msg {
        NetMessage::Handshake { filename, file_size, protocol_version } => {
            info!("Handshake from {}: file='{}', size={} bytes, protocol=v{}",
                  remote_addr, filename, file_size, protocol_version);

            if protocol_version != 1 {
                warn!("Protocol version mismatch from {}: expected v1, got v{}",
                      remote_addr, protocol_version);
                send_message(&mut send_stream, &NetMessage::error(
                    format!("Unsupported protocol version: {}", protocol_version)
                )).await?;
                return Err("Protocol version mismatch".into());
            }

            (filename, file_size)
        }
        _ => {
            error!("Invalid handshake from {}", remote_addr);
            send_message(&mut send_stream, &NetMessage::error(
                "Expected Handshake message".to_string()
            )).await?;
            return Err("Invalid handshake".into());
        }
    };

    let storage_dir = PathBuf::from("./server_storage");
    create_dir_all(&storage_dir)?;
    let file_path = storage_dir.join(&filename);
    let has_old_file = file_path.exists();

    send_message(&mut send_stream, &NetMessage::handshake_ack(has_old_file)).await?;

    if has_old_file {
        info!("Base file found for '{}'. Starting delta mode", filename);

        let sig_start = Instant::now();
        let signatures = generate_signatures(&file_path)?;
        let sig_duration = sig_start.elapsed();
        info!("Generated {} signatures in {:.2}s", signatures.len(), sig_duration.as_secs_f64());

        send_message(&mut send_stream, &NetMessage::RequestSignature).await?;

        for chunk in signatures.chunks(SIGNATURE_BATCH_SIZE) {
            let packet = NetMessage::signature_packet(chunk.to_vec());
            send_message(&mut send_stream, &packet).await?;
        }

        send_message(&mut send_stream, &NetMessage::SignatureEnd).await?;
        info!("All signatures sent to {}", remote_addr);

        let mut data_stream = connection.accept_uni().await?;

        let start_msg = receive_message(&mut data_stream).await?;
        if !matches!(start_msg, NetMessage::StartPatch) {
            error!("Expected StartPatch from {}", remote_addr);
            return Err("Expected StartPatch message".into());
        }

        info!("Receiving delta patch from {}...", remote_addr);
        let patch_start = Instant::now();

        let temp_path = storage_dir.join(format!("{}.tmp", filename));
        apply_patch_from_stream(&file_path, &mut data_stream, &temp_path).await?;

        let patch_duration = patch_start.elapsed();
        rename(&temp_path, &file_path)?;

        let final_size = std::fs::metadata(&file_path)?.len();
        let throughput_mb = (final_size as f64 / 1_000_000.0) / patch_duration.as_secs_f64().max(0.001);

        info!("File '{}' updated successfully", filename);
        info!("Transfer complete: {:.2} MB/s, duration: {:.2}s", throughput_mb, patch_duration.as_secs_f64());

        let checksum = compute_file_checksum(&file_path)?;
        send_message(&mut send_stream, &NetMessage::verify_result(true, checksum)).await?;
        send_stream.finish()?;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    } else {
        warn!("Base file missing for '{}'. Fallback to full upload", filename);

        let mut data_stream = connection.accept_uni().await?;

        let temp_path = storage_dir.join(format!("{}.tmp", filename));
        let mut file = BufWriter::new(File::create(&temp_path)?);
        let mut buffer = vec![0u8; BUFFER_SIZE];
        let mut total_received = 0u64;

        let upload_start = Instant::now();

        loop {
            match data_stream.read(&mut buffer).await {
                Ok(Some(n)) => {
                    file.write_all(&buffer[..n])?;
                    total_received += n as u64;
                }
                Ok(None) => break,
                Err(e) => {
                    error!("Stream read error from {}: {}", remote_addr, e);
                    return Err(format!("Stream read error: {}", e).into());
                }
            }
        }

        file.flush()?;
        drop(file);

        if total_received != file_size {
            error!("Size mismatch from {}: expected {}, received {}",
                   remote_addr, file_size, total_received);
            return Err(format!(
                "Size mismatch: expected {}, received {}",
                file_size, total_received
            ).into());
        }

        let upload_duration = upload_start.elapsed();
        let throughput_mb = (total_received as f64 / 1_000_000.0) / upload_duration.as_secs_f64().max(0.001);

        rename(&temp_path, &file_path)?;
        info!("File '{}' uploaded successfully: {} bytes", filename, total_received);
        info!("Upload complete: {:.2} MB/s, duration: {:.2}s", throughput_mb, upload_duration.as_secs_f64());

        let checksum = compute_file_checksum(&file_path)?;
        send_message(&mut send_stream, &NetMessage::verify_result(true, checksum)).await?;
        send_stream.finish()?;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let total_duration = start_time.elapsed();
    info!("Connection from {} closed (total: {:.2}s)\n", remote_addr, total_duration.as_secs_f64());
    Ok(())
}

async fn receive_message(stream: &mut quinn::RecvStream) -> Result<NetMessage, Box<dyn std::error::Error>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let msg_len = u32::from_le_bytes(len_buf) as usize;

    let mut msg_buf = vec![0u8; msg_len];
    stream.read_exact(&mut msg_buf).await?;

    Ok(NetMessage::deserialize(&msg_buf)?)
}

async fn send_message(stream: &mut quinn::SendStream, msg: &NetMessage) -> Result<(), Box<dyn std::error::Error>> {
    let data = msg.serialize()?;
    let len = data.len() as u32;

    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(&data).await?;

    Ok(())
}

fn generate_signatures(file_path: &Path) -> io::Result<Vec<ChunkSignature>> {
    let file = File::open(file_path)?;
    let mut chunker = Chunker::new(file);
    let mut signatures = Vec::new();

    while let Some(chunk) = chunker.next_chunk()? {
        let weak_hash = compute_weak_hash(&chunk.data);
        let strong_hash = compute_strong_hash(&chunk.data);

        signatures.push(ChunkSignature::new(
            chunk.offset,
            chunk.length,
            weak_hash,
            strong_hash,
        ));
    }

    Ok(signatures)
}

#[inline]
fn compute_weak_hash(data: &[u8]) -> u64 {
    let mut hash = 0u64;
    for &byte in data {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
    }
    hash
}

#[inline]
fn compute_strong_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

async fn apply_patch_from_stream(
    old_file_path: &Path,
    stream: &mut quinn::RecvStream,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let old_file = File::open(old_file_path)?;
    let output_file = File::create(output_path)?;
    let output_writer = BufWriter::new(output_file);

    let mut patch_builder = StreamingPatchBuilder::new(old_file, output_writer);

    loop {
        let mut tag = [0u8; 1];
        match stream.read_exact(&mut tag).await {
            Ok(_) => {},
            Err(quinn::ReadExactError::FinishedEarly(_)) => break,
            Err(e) => return Err(e.into()),
        }

        let instruction = read_instruction_async(tag[0], stream).await?;
        patch_builder.apply_instruction(&instruction)?;
    }

    patch_builder.finalize()?;
    Ok(())
}

async fn read_instruction_async(
    tag: u8,
    stream: &mut quinn::RecvStream,
) -> io::Result<PatchInstruction> {
    match tag {
        0x01 => {
            let mut offset_buf = [0u8; 8];
            let mut length_buf = [0u8; 8];
            stream.read_exact(&mut offset_buf).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            stream.read_exact(&mut length_buf).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            let offset = u64::from_le_bytes(offset_buf);
            let length = u64::from_le_bytes(length_buf) as usize;
            Ok(PatchInstruction::Copy(offset, length))
        }
        0x02 => {
            let mut length_buf = [0u8; 8];
            stream.read_exact(&mut length_buf).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            let length = u64::from_le_bytes(length_buf) as usize;
            let mut data = vec![0u8; length];
            stream.read_exact(&mut data).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            Ok(PatchInstruction::Literal(data))
        }
        0x03 => {
            let mut decompressed_len_buf = [0u8; 8];
            let mut compressed_len_buf = [0u8; 4];
            stream.read_exact(&mut decompressed_len_buf).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            stream.read_exact(&mut compressed_len_buf).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            let decompressed_len = u64::from_le_bytes(decompressed_len_buf);
            let compressed_len = u32::from_le_bytes(compressed_len_buf) as usize;
            let mut compressed_data = vec![0u8; compressed_len];
            stream.read_exact(&mut compressed_data).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            Ok(PatchInstruction::CompressedLiteral {
                decompressed_len,
                compressed_data,
            })
        }
        0x04 => {
            let mut length_buf = [0u8; 8];
            stream.read_exact(&mut length_buf).await
                .map_err(|e| io::Error::new(io::ErrorKind::UnexpectedEof, e))?;
            let length = u64::from_le_bytes(length_buf);
            Ok(PatchInstruction::Skip(length))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Unknown patch instruction tag: {}", tag),
        )),
    }
}

fn compute_file_checksum(file_path: &Path) -> io::Result<[u8; 32]> {
    let file = File::open(file_path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(*hasher.finalize().as_bytes())
}
