use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

use crate::jitter::{Jitter, Player};
use crate::rtp;

pub struct Options {
    pub port: u16,
    pub latency_ms: u32,
    pub rate: u32,
    pub channels: usize,
}

/// Receive RTP audio on a UDP port and play it on the default sound card until killed.
pub fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let device = cpal::default_host().default_output_device().ok_or("no sound output device")?;
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let target = (options.rate * options.latency_ms / 1000).max(1) as usize;
    let jitter = Arc::new(Mutex::new(Jitter::new(target)));

    let stream = match format {
        SampleFormat::F32 => play::<f32>(&device, &config, &jitter, options.rate),
        SampleFormat::I16 => play::<i16>(&device, &config, &jitter, options.rate),
        SampleFormat::U16 => play::<u16>(&device, &config, &jitter, options.rate),
        SampleFormat::I32 => play::<i32>(&device, &config, &jitter, options.rate),
        SampleFormat::F64 => play::<f64>(&device, &config, &jitter, options.rate),
        other => return Err(format!("sound card sample format {other} is not supported").into()),
    }?;
    stream.play()?;

    let socket = UdpSocket::bind(("0.0.0.0", options.port))?;
    println!(
        "Listening on UDP port {} ({} Hz, {} ch, {} ms buffer); sound card: {} Hz, {} ch, {format}",
        options.port, options.rate, options.channels, options.latency_ms, config.sample_rate, config.channels
    );
    println!("Ctrl+C to quit.");

    let mut buf = [0u8; 65536];
    let mut sender: Option<SocketAddr> = None;
    let mut last_report = Instant::now();
    let mut reported_underruns = 0;
    loop {
        let (len, from) = socket.recv_from(&mut buf)?;
        let Some(packet) = rtp::parse(&buf[..len]) else { continue };
        if sender != Some(from) {
            println!("Receiving from {from}");
            sender = Some(from);
        }
        let mut jitter = jitter.lock().unwrap();
        jitter.push(packet.sequence, packet.payload, options.channels);
        if last_report.elapsed() > Duration::from_secs(10) && jitter.underruns != reported_underruns {
            println!("Buffer ran dry {} times; try a bigger --latency", jitter.underruns - reported_underruns);
            reported_underruns = jitter.underruns;
            last_report = Instant::now();
        }
    }
}

fn play<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    jitter: &Arc<Mutex<Jitter>>,
    input_rate: u32,
) -> Result<cpal::Stream, Box<dyn Error>>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut player = Player::new(input_rate, config.sample_rate);
    let jitter = jitter.clone();
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _| {
            let mut jitter = jitter.lock().unwrap();
            player.update_speed(&jitter);
            for frame in data.chunks_mut(channels) {
                let [left, right] = player.next_frame(&mut jitter);
                for (channel, sample) in frame.iter_mut().enumerate() {
                    let value = match (channel, channels) {
                        (_, 1) => (left + right) * 0.5,
                        (0, _) => left,
                        (1, _) => right,
                        _ => 0.0,
                    };
                    *sample = T::from_sample(value);
                }
            }
        },
        |err| eprintln!("sound card: {err}"),
        None,
    )?;
    Ok(stream)
}
