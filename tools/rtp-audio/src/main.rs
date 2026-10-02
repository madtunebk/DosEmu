//! Play sound from other machines (WSL, Linux boxes) on this one, over the network as RTP.
//!
//!   rtp-audio [receive] [--port 46000] [--latency 60] [--rate 48000] [--channels 2]
//!   parec ... | rtp-audio send <host:port> [--rate 48000] [--channels 2]

mod jitter;
mod receive;
mod rtp;
mod send;

use std::process::ExitCode;

const USAGE: &str = "\
usage:
  rtp-audio [receive] [--port 46000] [--latency 60] [--rate 48000] [--channels 2]
      play RTP audio (16-bit PCM) arriving on a UDP port; --latency is the buffer in ms
  rtp-audio send <host:port> [--rate 48000] [--channels 2]
      send raw big-endian 16-bit PCM from stdin, e.g.
      parec -d windows.monitor --raw --format=s16be --rate=48000 --channels=2 --latency-msec=20 \\
        | rtp-audio send 172.20.0.1:46000   (send.sh does this for you)";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("rtp-audio: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let (mut command, mut destination) = (None, None);
    let (mut port, mut latency_ms, mut rate, mut channels) = (46000u16, 60u32, 48000u32, 2usize);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--port" => port = value()?.parse()?,
            "--latency" => latency_ms = value()?.parse()?,
            "--rate" => rate = value()?.parse()?,
            "--channels" => channels = value()?.parse()?,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "receive" | "send" if command.is_none() => command = Some(arg),
            _ if command.as_deref() == Some("send") && destination.is_none() => destination = Some(arg),
            _ => return Err(format!("unexpected argument '{arg}'\n{USAGE}").into()),
        }
    }
    if !(1..=2).contains(&channels) {
        return Err("--channels must be 1 or 2".into());
    }
    if rate == 0 || latency_ms == 0 {
        return Err("--rate and --latency must be above 0".into());
    }
    match command.as_deref() {
        Some("send") => send::run(&destination.ok_or(format!("send needs <host:port>\n{USAGE}"))?, rate, channels),
        _ => receive::run(receive::Options { port, latency_ms, rate, channels }),
    }
}
