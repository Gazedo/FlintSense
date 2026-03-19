//! flint-debug — LoRa receive + mesh relay firmware for Heltec WiFi LoRa 32 V2.
//!
//! ## Bring-up checklist
//!
//!   1. Flash the Heltec Arduino LoRa TX example on a second V2 board.
//!   2. Note the exact SF / BW / CR / frequency from that sketch and set the
//!      constants below to match.  Arduino-LoRa defaults are shown inline.
//!   3. Flash this firmware: `cargo run --release` (from this crate directory).
//!   4. Any received packet that doesn't decode as a MeshEnvelope is hex-dumped
//!      so you can inspect raw Arduino packets before full Rust TX is ready.
//!
//! ## Heltec V2 pin mapping
//!
//!   SPI2 (SX1276): SCK GPIO5  MISO GPIO19  MOSI GPIO27  CS GPIO18  RST GPIO14  DIO0 GPIO26
//!   I2C0 (OLED):   SDA GPIO4  SCL GPIO15   RST GPIO16
//!
//! ## Meshtastic US LongFast (future interop target)
//!
//!   freq=906_875_000  SF11  BW250  CR4/8  use_public_network=true

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Delay, Timer};
use embedded_graphics::{
    mono_font::{MonoTextStyleBuilder, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::{
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use flint_proto::{FlintPayload, MAX_PACKET_BYTES, SeenCache, decode, encode};
use log::{error, info, warn};
use lora_phy::{
    LoRa, RxMode,
    iv::GenericSx127xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx127x::{Config as Sx127xConfig, Sx127x, Sx1276},
};
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};

esp_bootloader_esp_idf::esp_app_desc!();

// ── LoRa channel config ───────────────────────────────────────────────────────
//
// Match these to your Arduino TX sketch.
// Arduino-LoRa library defaults (shown for reference):
//   LoRa.begin(915E6)              → LORA_FREQUENCY_HZ = 915_000_000
//   LoRa.setSpreadingFactor(7)     → SpreadingFactor::_7
//   LoRa.setSignalBandwidth(125E3) → Bandwidth::_125KHz
//   LoRa.setCodingRate4(5)         → CodingRate::_4_5
//   LoRa.setSyncWord(0x12)         → use_public_network = false

const LORA_FREQUENCY_HZ: u32 = 915_000_000;
const LORA_SF: SpreadingFactor = SpreadingFactor::_7;
const LORA_BW: Bandwidth = Bandwidth::_125KHz;
const LORA_CR: CodingRate = CodingRate::_4_5;
const LORA_PUBLIC_NETWORK: bool = false;
const LORA_TX_POWER_DBM: i32 = 14;

// ── Display / formatting helpers ──────────────────────────────────────────────

/// Format "RX: {count}" into `buf` without heap allocation.
fn fmt_count(buf: &mut [u8; 16], count: u32) -> &str {
    buf[..4].copy_from_slice(b"RX: ");
    let mut digits = [0u8; 10];
    let mut ndig = 0usize;
    let mut n = count;
    if n == 0 {
        digits[0] = b'0';
        ndig = 1;
    } else {
        while n > 0 {
            digits[ndig] = b'0' + (n % 10) as u8;
            n /= 10;
            ndig += 1;
        }
    }
    let mut pos = 4usize;
    for i in (0..ndig).rev() {
        buf[pos] = digits[i];
        pos += 1;
    }
    core::str::from_utf8(&buf[..pos]).unwrap()
}

/// Format a byte slice as space-separated hex into `buf` (max 32 bytes → 95 chars).
fn fmt_hex<'a>(buf: &'a mut [u8; 95], data: &[u8]) -> &'a str {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut pos = 0;
    for &b in data {
        if pos + 3 > buf.len() {
            break;
        }
        buf[pos] = HEX[(b >> 4) as usize];
        buf[pos + 1] = HEX[(b & 0xf) as usize];
        buf[pos + 2] = b' ';
        pos += 3;
    }
    if pos > 0 {
        pos -= 1; // trim trailing space
    }
    core::str::from_utf8(&buf[..pos]).unwrap_or("")
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // Initialise the log backend — reads ESP_LOG env var set in .cargo/config.toml.
    esp_println::logger::init_logger_from_env();

    info!("flint-debug booting");

    // ── OLED (SSD1306 128×64 via I2C0, SDA=GPIO4, SCL=GPIO15, RST=GPIO16) ───

    let mut oled_rst = Output::new(peripherals.GPIO16, Level::High, OutputConfig::default());
    oled_rst.set_low();
    Timer::after_millis(10).await;
    oled_rst.set_high();
    Timer::after_millis(10).await;

    let i2c = I2c::new(peripherals.I2C0, I2cConfig::default())
        .unwrap()
        .with_sda(peripherals.GPIO4)
        .with_scl(peripherals.GPIO15);

    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    // Boot screen
    display.clear(BinaryColor::Off).unwrap();
    Text::with_baseline(
        "FlintMesh Debug",
        Point::new(0, 0),
        text_style,
        Baseline::Top,
    )
    .draw(&mut display)
    .unwrap();
    Text::with_baseline("RX: 0", Point::new(0, 20), text_style, Baseline::Top)
        .draw(&mut display)
        .unwrap();
    display.flush().unwrap();

    let mut rx_count: u32 = 0;

    // ── SPI2 for SX1276 ──────────────────────────────────────────────────────

    let cs = Output::new(peripherals.GPIO18, Level::High, OutputConfig::default());

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(1))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO5)
    .with_miso(peripherals.GPIO19)
    .with_mosi(peripherals.GPIO27)
    .into_async();

    // ExclusiveDevice wraps SpiBus + CS pin into the SpiDevice trait lora-phy needs.
    let spi_dev = ExclusiveDevice::new(spi, cs, Delay).unwrap();

    // ── SX1276 interface variant (reset + DIO0 IRQ + optional RF switches) ────

    let reset = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    let dio0 = Input::new(
        peripherals.GPIO26,
        InputConfig::default().with_pull(Pull::Down),
    );

    let iv = GenericSx127xInterfaceVariant::new(
        reset, dio0, None, // rf_switch_rx — not needed on Heltec V2
        None, // rf_switch_tx — not needed on Heltec V2
    )
    .unwrap();

    // ── Radio init ────────────────────────────────────────────────────────────

    let sx_config = Sx127xConfig {
        chip: Sx1276,
        tcxo_used: false,
        tx_boost: false, // PA_BOOST pin — false uses RFO for lower power (<14 dBm)
        rx_boost: false,
    };

    let mut lora = LoRa::new(
        Sx127x::new(spi_dev, iv, sx_config),
        LORA_PUBLIC_NETWORK,
        Delay,
    )
    .await
    .expect("LoRa init failed — check SPI wiring and pin assignments");

    // ── Modulation + packet params ────────────────────────────────────────────

    let mdm = lora
        .create_modulation_params(LORA_SF, LORA_BW, LORA_CR, LORA_FREQUENCY_HZ)
        .expect("invalid modulation params");

    let rx_params = lora
        .create_rx_packet_params(8, false, MAX_PACKET_BYTES as u8, true, false, &mdm)
        .expect("invalid rx packet params");

    let mut tx_params = lora
        .create_tx_packet_params(8, false, true, false, &mdm)
        .expect("invalid tx packet params");

    let mut seen = SeenCache::new();

    info!(
        "RX loop — {}Hz  SF{}  BW{:?}  CR{:?}",
        LORA_FREQUENCY_HZ, LORA_SF as u8, LORA_BW, LORA_CR,
    );

    // ── Main receive loop ─────────────────────────────────────────────────────

    loop {
        if let Err(e) = lora
            .prepare_for_rx(RxMode::Continuous, &mdm, &rx_params)
            .await
        {
            error!("prepare_for_rx: {:?}", e);
            continue;
        }

        let mut rx_buf = [0u8; MAX_PACKET_BYTES];

        let (len, status) = match lora.rx(&rx_params, &mut rx_buf).await {
            Ok(r) => r,
            Err(e) => {
                warn!("rx: {:?}", e);
                continue;
            }
        };

        rx_count = rx_count.saturating_add(1);

        // Update OLED counter
        let mut count_buf = [0u8; 16];
        let count_str = fmt_count(&mut count_buf, rx_count);
        display.clear(BinaryColor::Off).unwrap();
        Text::with_baseline(
            "FlintMesh Debug",
            Point::new(0, 0),
            text_style,
            Baseline::Top,
        )
        .draw(&mut display)
        .unwrap();
        Text::with_baseline(count_str, Point::new(0, 20), text_style, Baseline::Top)
            .draw(&mut display)
            .unwrap();
        display.flush().unwrap();

        let raw = &rx_buf[..len as usize];
        info!(
            "RX {} bytes  RSSI={}dBm  SNR={}dB",
            len, status.rssi, status.snr
        );

        // ── Decode ────────────────────────────────────────────────────────────

        match decode(raw) {
            Ok(envelope) => {
                info!(
                    "  FlintPacket  from=0x{:08x}  id=0x{:08x}  hops={}/{}",
                    envelope.from, envelope.packet_id, envelope.hop_limit, envelope.hop_start,
                );

                match &envelope.payload {
                    FlintPayload::SensorReading(w) => {
                        info!(
                            "  node={}  {}.{:02}C  {}%RH  {}x0.5m/s@{}deg  \
                             fuel={}%  batt={}%/{}mV  seq={}",
                            w.node_id,
                            w.temp_c / 100,
                            w.temp_c.unsigned_abs() % 100,
                            w.humidity_pct,
                            w.wind_speed_ms,
                            w.wind_dir_deg,
                            w.fuel_moisture,
                            w.battery_soc,
                            w.battery_mv,
                            w.sequence,
                        );
                    }
                }

                // ── Mesh relay ────────────────────────────────────────────────

                if seen.check_and_insert(envelope.packet_id) {
                    match envelope.for_relay() {
                        Some(relay) => {
                            let mut tx_buf = [0u8; MAX_PACKET_BYTES];
                            match encode(&relay, &mut tx_buf) {
                                Ok(bytes) => {
                                    match lora
                                        .prepare_for_tx(
                                            &mdm,
                                            &mut tx_params,
                                            LORA_TX_POWER_DBM,
                                            bytes,
                                        )
                                        .await
                                    {
                                        Ok(_) => match lora.tx().await {
                                            Ok(_) => info!(
                                                "  RELAY id=0x{:08x}  hops_left={}",
                                                relay.packet_id, relay.hop_limit
                                            ),
                                            Err(e) => warn!("tx: {:?}", e),
                                        },
                                        Err(e) => warn!("prepare_for_tx: {:?}", e),
                                    }
                                }
                                Err(e) => warn!("encode: {:?}", e),
                            }
                        }
                        None => info!("  hop_limit=0 — local delivery only"),
                    }
                } else {
                    info!("  duplicate packet_id — discarding");
                }
            }

            Err(_) => {
                // Not a FlintPacket — probably the Arduino default sketch or noise.
                // Hex-dump to see what the transmitter is actually sending.
                let dump = raw.len().min(32);
                let mut hex_buf = [0u8; 95];
                info!("  unknown format — {}", fmt_hex(&mut hex_buf, &raw[..dump]));
            }
        }
    }
}
