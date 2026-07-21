// Verify type_hash and crc32 values match what ipc_lowering.rs uses.
use vuma_codegen::ipc;

fn main() {
    let th = ipc::type_hash("i64");
    println!("type_hash(\"i64\") = 0x{:016x}", th);
    println!("expected          = 0x2ae1af192b331746");

    // Frame for payload=42, seq=0, channel_id=0, cap_count=0
    let mut frame = [0u8; 56];
    frame[0..4].copy_from_slice(&0x414D5556u32.to_le_bytes());
    frame[4..8].copy_from_slice(&0x00020000u32.to_le_bytes());
    // [8..16] channel_id = 0
    // [16..24] sequence = 0
    frame[24..32].copy_from_slice(&th.to_le_bytes());
    frame[32..36].copy_from_slice(&8u32.to_le_bytes());
    // [36..40] cap_count = 0
    // [40..44] reserved = 0
    frame[44..52].copy_from_slice(&42i64.to_le_bytes());
    let crc = ipc::crc32(&frame[0..52]);
    println!("crc32(frame[0..52]) for payload=42, seq=0 = 0x{:08x}", crc);

    // Frame for payload=42, seq=1
    let mut frame2 = frame;
    frame2[16..24].copy_from_slice(&1i64.to_le_bytes());
    let crc2 = ipc::crc32(&frame2[0..52]);
    println!("crc32(frame[0..52]) for payload=42, seq=1 = 0x{:08x}", crc2);
}
