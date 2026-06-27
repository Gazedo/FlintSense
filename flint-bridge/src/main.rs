//! flint-bridge — LoRa receive + mesh relay + USB bridge for Heltec WiFi LoRa 32 V2.
//!
//! Receives `MeshEnvelope` packets off the air, relays unseen ones, and reports
//! every reception to the host over USB as self-delimiting binary records (see
//! `flint-proto`'s `bridge` module). `flint-gateway` decodes the stream.
//!
//! Two tasks: `main` runs the radio + USB, and `display_task` owns the OLED and
//! redraws from a channel. Everything is async — UART (`write_async`), radio, and
//! the OLED I2C (`flush().await`) — so each yields the cooperative executor. A
//! *blocking* I2C flush in a task would stall the executor and break the radio.
//!
//! ## Heltec V2 pin mapping
//!
//!   SPI2 (SX1276): SCK GPIO5  MISO GPIO19  MOSI GPIO27  CS GPIO18  RST GPIO14  DIO0 GPIO26
//!   I2C0 (OLED):   SDA GPIO4  SCL GPIO15   RST GPIO16
//!   UART0 (USB):   TX GPIO1   (CP2102 bridge)

#![no_std]
#![no_main]

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Delay, Instant, Timer};
use embedded_graphics::{
    mono_font::{MonoTextStyle, MonoTextStyleBuilder, ascii::FONT_5X8},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
    text::{Baseline, Text},
};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::{
    Async,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, UartTx},
};
use flint_proto::{
    BRIDGE_MAX_FRAME, Disposition, FlintPayload, LinkQuality, MAX_PACKET_BYTES, SeenCache, decode,
    encode, encode_frame_report, encode_raw_frame,
};
use heapless::String;
use lora_phy::{
    LoRa, RxMode,
    iv::GenericSx127xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx127x::{Config as Sx127xConfig, Sx127x, Sx1276},
};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, mode::BufferedGraphicsModeAsync, prelude::*};

esp_bootloader_esp_idf::esp_app_desc!();

// ── LoRa channel config ───────────────────────────────────────────────────────
//
// Match these to your transmitter (Arduino-LoRa library defaults shown):
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

/// Concrete type of the buffered OLED over async I2C.
type Oled = Ssd1306Async<
    I2CInterface<I2c<'static, Async>>,
    DisplaySize128x64,
    BufferedGraphicsModeAsync<DisplaySize128x64>,
>;

/// Details of the most recent decoded reception, for the OLED.
struct MsgRx {
    node_id: u32,
    rssi: i16,
    timestamp_s: u64,
    packet_id: u32,
    temp_c: i16,
    humidity: u8,
    wind_speed_ms: u8,
    gas_ohms: u32,
    battery_soc: u8,
}

/// Display update events. Counts are authoritative (from the RX loop) and carried
/// in every event, so a dropped event only skips a redraw, never miscounts.
enum DisplayEvent {
    Decoded { decoded: u32, errors: u32, msg: MsgRx },
    Counts { decoded: u32, errors: u32 },
}

static DISPLAY: Channel<CriticalSectionRawMutex, DisplayEvent, 8> = Channel::new();

// ── USB output ────────────────────────────────────────────────────────────────

/// Write all bytes to the USB UART (async).
async fn usb_write_all(usb_tx: &mut UartTx<'static, Async>, bytes: &[u8]) {
    let mut sent = 0;
    while sent < bytes.len() {
        match usb_tx.write_async(&bytes[sent..]).await {
            Ok(0) => break,
            Ok(n) => sent += n,
            Err(_) => break,
        }
    }
}

/// Emit a decoded reception as a binary `FrameReport`.
async fn report_frame(
    usb_tx: &mut UartTx<'static, Async>,
    link: &LinkQuality,
    disposition: Disposition,
    hops_left: u8,
    envelope_bytes: &[u8],
) {
    let mut frame = [0u8; BRIDGE_MAX_FRAME];
    if let Ok(n) = encode_frame_report(link, disposition, hops_left, envelope_bytes, &mut frame) {
        usb_write_all(usb_tx, &frame[..n]).await;
    }
}

/// Emit an undecodable reception as a binary `RawFrame`.
async fn report_raw(usb_tx: &mut UartTx<'static, Async>, link: &LinkQuality, raw: &[u8]) {
    let mut frame = [0u8; BRIDGE_MAX_FRAME];
    if let Ok(n) = encode_raw_frame(link, raw, &mut frame) {
        usb_write_all(usb_tx, &frame[..n]).await;
    }
}

// ── Display task ──────────────────────────────────────────────────────────────

#[embassy_executor::task]
async fn display_task(mut display: Oled) {
    let mut decoded = 0u32;
    let mut errors = 0u32;
    let mut last: Option<MsgRx> = None;

    render(&mut display, decoded, errors, &last).await;

    loop {
        match DISPLAY.receive().await {
            DisplayEvent::Decoded {
                decoded: d,
                errors: e,
                msg,
            } => {
                decoded = d;
                errors = e;
                last = Some(msg);
            }
            DisplayEvent::Counts {
                decoded: d,
                errors: e,
            } => {
                decoded = d;
                errors = e;
            }
        }
        render(&mut display, decoded, errors, &last).await;
    }
}

/// Layout (128×64):
///
///   FlintBridge RX12 ERR0      ← title + metrics on one line (FONT_5X8)
///   ─────────────────────────
///   Pkt deadbeef               ← packet id, spans both columns
///   Node 7   │ T 23.50C        ← two columns
///   RSSI -91 │ H 42%
///   Time 5s  │ W 7.0
///   Batt 88% │ Gas 51234
async fn render(display: &mut Oled, decoded: u32, errors: u32, last: &Option<MsgRx>) {
    let font: MonoTextStyle<BinaryColor> = MonoTextStyleBuilder::new()
        .font(&FONT_5X8)
        .text_color(BinaryColor::On)
        .build();
    let rule = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    display.clear(BinaryColor::Off).ok();

    let mut buf: String<28> = String::new();

    // ── Header: title + metrics on one line ──
    let _ = write!(buf, "FlintBridge RX{decoded} ERR{errors}");
    Text::with_baseline(buf.as_str(), Point::new(0, 0), font, Baseline::Top)
        .draw(display)
        .ok();
    Line::new(Point::new(0, 10), Point::new(127, 10))
        .into_styled(rule)
        .draw(display)
        .ok();

    if let Some(m) = last {
        // ── Packet id header (spans both columns) ──
        buf.clear();
        let _ = write!(buf, "Pkt {:08x}", m.packet_id);
        Text::with_baseline(buf.as_str(), Point::new(0, 12), font, Baseline::Top)
            .draw(display)
            .ok();

        // Column divider
        Line::new(Point::new(63, 21), Point::new(63, 63))
            .into_styled(rule)
            .draw(display)
            .ok();

        let rows = [22, 32, 42, 52];
        let cell = |display: &mut Oled, x: i32, row: usize, text: &str| {
            Text::with_baseline(text, Point::new(x, rows[row]), font, Baseline::Top)
                .draw(display)
                .ok();
        };

        // Left column: packet/link metrics
        buf.clear();
        let _ = write!(buf, "Node {}", m.node_id);
        cell(display, 0, 0, buf.as_str());
        buf.clear();
        let _ = write!(buf, "RSSI {}", m.rssi);
        cell(display, 0, 1, buf.as_str());
        buf.clear();
        let _ = write!(buf, "Time {}s", m.timestamp_s);
        cell(display, 0, 2, buf.as_str());
        buf.clear();
        let _ = write!(buf, "Batt {}%", m.battery_soc);
        cell(display, 0, 3, buf.as_str());

        // Right column: sensor readings
        buf.clear();
        let _ = write!(buf, "T {}.{:02}C", m.temp_c / 100, m.temp_c.unsigned_abs() % 100);
        cell(display, 67, 0, buf.as_str());
        buf.clear();
        let _ = write!(buf, "H {}%", m.humidity);
        cell(display, 67, 1, buf.as_str());
        buf.clear();
        let _ = write!(buf, "W {}.{}m/s", m.wind_speed_ms / 2, (m.wind_speed_ms % 2) * 5);
        cell(display, 67, 2, buf.as_str());
        buf.clear();
        let _ = write!(buf, "Gas {}", m.gas_ohms);
        cell(display, 67, 3, buf.as_str());
    } else {
        Text::with_baseline("waiting for RX...", Point::new(0, 14), font, Baseline::Top)
            .draw(display)
            .ok();
    }

    display.flush().await.ok();
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // ── USB serial output (UART0 TX on GPIO1, async). ─────────────────────────

    let mut usb_tx = UartTx::new(
        peripherals.UART0,
        UartConfig::default().with_baudrate(921_600),
    )
    .expect("UART0 init failed")
    .with_tx(peripherals.GPIO1)
    .into_async();

    // ── OLED (SSD1306 128×64 via async I2C0, SDA=GPIO4, SCL=GPIO15, RST=GPIO16) ─

    let mut oled_rst = Output::new(peripherals.GPIO16, Level::High, OutputConfig::default());
    oled_rst.set_low();
    Timer::after_millis(10).await;
    oled_rst.set_high();
    Timer::after_millis(10).await;

    let i2c = I2c::new(peripherals.I2C0, I2cConfig::default())
        .unwrap()
        .with_sda(peripherals.GPIO4)
        .with_scl(peripherals.GPIO15)
        .into_async();

    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306Async::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().await.unwrap();
    spawner.spawn(display_task(display)).ok();

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

    // ── SX1276 interface variant (reset + DIO0 IRQ) ───────────────────────────

    let reset = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    let dio0 = Input::new(
        peripherals.GPIO26,
        InputConfig::default().with_pull(Pull::Down),
    );

    let iv = GenericSx127xInterfaceVariant::new(reset, dio0, None, None).unwrap();

    // ── Radio init ────────────────────────────────────────────────────────────

    let sx_config = Sx127xConfig {
        chip: Sx1276,
        tcxo_used: false,
        tx_boost: false,
        rx_boost: false,
    };

    let mut lora = LoRa::new(Sx127x::new(spi_dev, iv, sx_config), LORA_PUBLIC_NETWORK, Delay)
        .await
        .expect("LoRa init failed — check SPI wiring and pin assignments");

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

    // Boot banner over USB (plain ASCII; the gateway skips it and resyncs on the
    // first framed record). A visible sign of life that TX works.
    {
        let mut banner: String<64> = String::new();
        let _ = write!(banner, "flint-bridge ready: {}Hz SF{}\r\n", LORA_FREQUENCY_HZ, LORA_SF as u8);
        usb_write_all(&mut usb_tx, banner.as_bytes()).await;
    }

    let mut decoded_count: u32 = 0;
    let mut error_count: u32 = 0;

    // ── Main receive loop ─────────────────────────────────────────────────────

    loop {
        if lora
            .prepare_for_rx(RxMode::Continuous, &mdm, &rx_params)
            .await
            .is_err()
        {
            continue;
        }

        let mut rx_buf = [0u8; MAX_PACKET_BYTES];
        let (len, status) = match lora.rx(&rx_params, &mut rx_buf).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        let raw = &rx_buf[..len as usize];
        let link = LinkQuality {
            rssi: status.rssi as i16,
            snr: status.snr as i16,
            len: len as u8,
        };

        match decode(raw) {
            Ok(envelope) => {
                let (disposition, hops_left) = if seen.check_and_insert(envelope.packet_id) {
                    match envelope.for_relay() {
                        Some(relay) => {
                            let mut tx_buf = [0u8; MAX_PACKET_BYTES];
                            if let Ok(bytes) = encode(&relay, &mut tx_buf)
                                && lora
                                    .prepare_for_tx(&mdm, &mut tx_params, LORA_TX_POWER_DBM, bytes)
                                    .await
                                    .is_ok()
                            {
                                let _ = lora.tx().await;
                            }
                            (Disposition::Relayed, relay.hop_limit)
                        }
                        None => (Disposition::LocalOnly, 0),
                    }
                } else {
                    (Disposition::Duplicate, 0)
                };

                report_frame(&mut usb_tx, &link, disposition, hops_left, raw).await;

                decoded_count = decoded_count.saturating_add(1);
                let FlintPayload::SensorReading(w) = &envelope.payload;
                let msg = MsgRx {
                    node_id: w.node_id as u32,
                    rssi: link.rssi,
                    timestamp_s: Instant::now().as_millis() / 1000,
                    packet_id: envelope.packet_id,
                    temp_c: w.temp_c,
                    humidity: w.humidity_pct,
                    wind_speed_ms: w.wind_speed_ms,
                    gas_ohms: w.gas_resistance_ohms,
                    battery_soc: w.battery_soc,
                };
                DISPLAY
                    .try_send(DisplayEvent::Decoded {
                        decoded: decoded_count,
                        errors: error_count,
                        msg,
                    })
                    .ok();
            }
            Err(_) => {
                report_raw(&mut usb_tx, &link, raw).await;
                error_count = error_count.saturating_add(1);
                DISPLAY
                    .try_send(DisplayEvent::Counts {
                        decoded: decoded_count,
                        errors: error_count,
                    })
                    .ok();
            }
        }
    }
}
