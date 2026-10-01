use std::error::Error;
use std::io::{ErrorKind, Read};
use std::net::UdpSocket;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::rtp;

/// 5 ms of audio per packet: small enough for low latency, ~1 KB so it never fragments.
const PACKET_MS: u32 = 5;

/// Read raw big-endian 16-bit PCM from stdin (e.g. `parec --format=s16be --raw`) and send it
/// as RTP to `destination` until stdin ends.
pub fn run(destination: &str, rate: u32, channels: usize) -> Result<(), Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(destination)?;
    let frames_per_packet = rate * PACKET_MS / 1000;
    let mut payload = vec![0u8; frames_per_packet as usize * channels * 2];
    let ssrc = SystemTime::now().duration_since(UNIX_EPOCH)?.subsec_nanos();
    let (mut sequence, mut timestamp) = (0u16, 0u32);
    eprintln!("Sending {rate} Hz, {channels} ch to {destination}");

    let mut stdin = std::io::stdin().lock();
    let mut packet = Vec::with_capacity(rtp::HEADER_LEN + payload.len());
    loop {
        match stdin.read_exact(&mut payload) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err.into()),
        }
        packet.clear();
        packet.extend_from_slice(&rtp::header(sequence, timestamp, ssrc));
        packet.extend_from_slice(&payload);
        // The receiver may not be running yet; keep sending.
        if let Err(err) = socket.send(&packet)
            && err.kind() != ErrorKind::ConnectionRefused
        {
            return Err(err.into());
        }
        sequence = sequence.wrapping_add(1);
        timestamp = timestamp.wrapping_add(frames_per_packet);
    }
}
