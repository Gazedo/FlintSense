//! flint-gateway — host-side reader for the flint-bridge USB stream.
//!
//! Opens the serial port the bridge is on, asserts binary mode, and decodes the
//! self-delimiting binary records with `flint-proto`, pretty-printing each one.
//! Stray bytes between records (boot text, panic backtraces) are skipped and the
//! parser resynchronises on the next valid, CRC-checked record.
//!
//! Usage: `flint-gateway [PORT] [BAUD]`
//!   PORT defaults to /dev/cu.usbserial-0001, BAUD to 921600.

use std::io::{Read, Write};
use std::time::Duration;

use flint_proto::{ParseOutcome, parse_record};

const DEFAULT_PORT: &str = "/dev/cu.usbserial-0001";
const DEFAULT_BAUD: u32 = 921_600;

/// Cap on the resync buffer — if we never find a record, drop the oldest bytes
/// rather than grow without bound.
const MAX_BUFFER: usize = 4096;

fn main() {
    let mut args = std::env::args().skip(1);
    let port = args.next().unwrap_or_else(|| DEFAULT_PORT.to_string());
    let baud: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_BAUD);

    eprintln!("flint-gateway: opening {port} @ {baud} baud");
    let mut serial = match serialport::new(&port, baud)
        .timeout(Duration::from_millis(200))
        .open()
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open {port}: {e}");
            std::process::exit(1);
        }
    };

    // Assert binary mode on the bridge so the stream is machine-parseable.
    if let Err(e) = serial.write_all(b"b") {
        eprintln!("warning: could not send mode command: {e}");
    }
    eprintln!("flint-gateway: reading bridge records (Ctrl-C to quit)");

    let mut acc: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        match serial.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                acc.extend_from_slice(&chunk[..n]);
                drain_records(&mut acc);
                if acc.len() > MAX_BUFFER {
                    let drop = acc.len() - 1024;
                    acc.drain(..drop);
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                eprintln!("serial read error: {e}");
                break;
            }
        }
    }
}

/// Parse and print every complete record at the front of `acc`, draining consumed
/// and skipped bytes until more input is needed.
fn drain_records(acc: &mut Vec<u8>) {
    loop {
        match parse_record(acc) {
            ParseOutcome::Record { record, consumed } => {
                println!("{record}");
                acc.drain(..consumed);
            }
            ParseOutcome::Skip(skip) => {
                acc.drain(..skip);
            }
            ParseOutcome::NeedMore => break,
        }
    }
}
