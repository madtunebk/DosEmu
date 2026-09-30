//! Guest sound for the browser: QEMU's `-audiodev dbus` backend hands the mixed output of the
//! sound cards (Sound Blaster 16, AdLib, PC speaker) to an AudioOutListener, and every chunk is
//! broadcast to the web clients as 16-bit PCM.

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::session::dbus_listener;

const AUDIO_PATH: &str = "/org/qemu/Display1/Audio";
const AUDIO_INTERFACE: &str = "org.qemu.Display1.Audio";
const LISTENER_PATH: &str = "/org/qemu/Display1/AudioOutListener";
/// Chunks buffered per client; a client further behind than this skips ahead (no growing lag).
const CLIENT_BACKLOG: usize = 32;

/// One message for the browser: an 8-byte header (sample rate u32 LE, channels u16 LE, 0u16)
/// followed by interleaved signed 16-bit little-endian samples.
pub type AudioChunk = Arc<Vec<u8>>;

/// Subscribe to get every chunk QEMU plays from now on.
#[derive(Clone)]
pub struct AudioOut {
    sender: broadcast::Sender<AudioChunk>,
}

impl AudioOut {
    pub fn subscribe(&self) -> broadcast::Receiver<AudioChunk> {
        self.sender.subscribe()
    }
}

pub fn start(dbus_address: &str) -> Result<AudioOut, Box<dyn Error>> {
    let (sender, _) = broadcast::channel(CLIENT_BACKLOG);
    let listener = OutListener { sender: sender.clone(), voices: Mutex::new(HashMap::new()) };
    let address = dbus_address.to_string();
    dbus_listener::run_on_own_thread("audio", move || async move {
        let bus = zbus::connection::Builder::address(address.as_str())?.build().await?;
        let peer =
            dbus_listener::register_p2p(&bus, AUDIO_PATH, AUDIO_INTERFACE, "RegisterOutListener", LISTENER_PATH, listener)
                .await?;
        Ok((bus, peer))
    })?;
    Ok(AudioOut { sender })
}

/// Sample format of one QEMU output voice, from Init.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Format {
    bits: u8,
    signed: bool,
    float: bool,
    big_endian: bool,
    rate: u32,
    channels: u8,
}

struct Voice {
    format: Format,
    enabled: bool,
    muted: bool,
}

struct OutListener {
    sender: broadcast::Sender<AudioChunk>,
    voices: Mutex<HashMap<u64, Voice>>,
}

/// Method names and signatures follow QEMU's org.qemu.Display1.AudioOutListener
/// (ui/dbus-display1.xml).
#[zbus::interface(name = "org.qemu.Display1.AudioOutListener")]
impl OutListener {
    #[allow(clippy::too_many_arguments)]
    async fn init(
        &self,
        id: u64,
        bits: u8,
        is_signed: bool,
        is_float: bool,
        freq: u32,
        nchannels: u8,
        _bytes_per_frame: u32,
        _bytes_per_second: u32,
        be: bool,
    ) {
        let format = Format { bits, signed: is_signed, float: is_float, big_endian: be, rate: freq, channels: nchannels };
        println!("audio: voice {id}: {freq} Hz, {nchannels} channel(s), {bits}-bit");
        self.voices.lock().unwrap().insert(id, Voice { format, enabled: false, muted: false });
    }

    async fn fini(&self, id: u64) {
        self.voices.lock().unwrap().remove(&id);
    }

    async fn set_enabled(&self, id: u64, enabled: bool) {
        if let Some(voice) = self.voices.lock().unwrap().get_mut(&id) {
            voice.enabled = enabled;
        }
    }

    async fn set_volume(&self, id: u64, mute: bool, _volume: Vec<u8>) {
        if let Some(voice) = self.voices.lock().unwrap().get_mut(&id) {
            voice.muted = mute;
        }
    }

    async fn write(&self, id: u64, data: Vec<u8>) {
        let format = match self.voices.lock().unwrap().get(&id) {
            Some(voice) if voice.enabled && !voice.muted => voice.format,
            _ => return,
        };
        // Nobody listening: skip the conversion.
        if self.sender.receiver_count() == 0 {
            return;
        }
        let Some(samples) = to_s16(format, &data) else {
            return eprintln!("audio: unsupported format {format:?}");
        };
        // Silence is sent as nothing; the page simply plays nothing until sound arrives.
        if samples.iter().all(|&sample| sample == 0) {
            return;
        }
        let mut chunk = Vec::with_capacity(8 + samples.len() * 2);
        chunk.extend_from_slice(&format.rate.to_le_bytes());
        chunk.extend_from_slice(&u16::from(format.channels).to_le_bytes());
        chunk.extend_from_slice(&[0, 0]);
        for sample in samples {
            chunk.extend_from_slice(&sample.to_le_bytes());
        }
        let _ = self.sender.send(Arc::new(chunk));
    }

    /// Optional extra interfaces; none.
    #[zbus(property)]
    async fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Convert interleaved samples to signed 16-bit. None for sample sizes QEMU doesn't produce.
fn to_s16(format: Format, data: &[u8]) -> Option<Vec<i16>> {
    let Format { bits, signed, float, big_endian, .. } = format;
    let bytes = usize::from(bits / 8);
    if ![1, 2, 4].contains(&bytes) || bits % 8 != 0 {
        return None;
    }
    let samples = data.chunks_exact(bytes).map(|raw| {
        let mut word = [0u8; 4];
        word[..bytes].copy_from_slice(raw);
        if big_endian {
            word[..bytes].reverse();
        }
        let value = u32::from_le_bytes(word);
        match (bytes, signed, float) {
            (4, _, true) => (f32::from_bits(value).clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16,
            (1, false, _) => ((value as i16) - 128) << 8,
            (1, true, _) => i16::from(value as u8 as i8) << 8,
            (2, false, _) => (value as u16 ^ 0x8000) as i16,
            (2, true, _) => value as u16 as i16,
            (4, false, _) => ((value ^ 0x8000_0000) >> 16) as u16 as i16,
            _ => (value >> 16) as u16 as i16,
        }
    });
    Some(samples.collect())
}

#[cfg(test)]
mod tests {
    use super::{Format, to_s16};

    fn format(bits: u8, signed: bool, float: bool, big_endian: bool) -> Format {
        Format { bits, signed, float, big_endian, rate: 44100, channels: 2 }
    }

    #[test]
    fn converts_common_formats() {
        assert_eq!(to_s16(format(16, true, false, false), &[0x34, 0x12, 0x00, 0x80]).unwrap(), [0x1234, i16::MIN]);
        assert_eq!(to_s16(format(16, true, false, true), &[0x12, 0x34]).unwrap(), [0x1234]);
        assert_eq!(to_s16(format(8, false, false, false), &[0x80, 0xff, 0x00]).unwrap(), [0, 127 << 8, -128 << 8]);
        assert_eq!(to_s16(format(16, false, false, false), &[0x00, 0x80]).unwrap(), [0]);
        assert_eq!(to_s16(format(32, true, true, false), &1.0f32.to_le_bytes()).unwrap(), [i16::MAX]);
        assert_eq!(to_s16(format(32, true, false, false), &i32::MIN.to_le_bytes()).unwrap(), [i16::MIN]);
        assert!(to_s16(format(24, true, false, false), &[0; 3]).is_none());
    }
}
