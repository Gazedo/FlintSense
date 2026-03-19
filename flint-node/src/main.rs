//! flint-node — LoRa transmit test firmware for Heltec WiFi LoRa 32 V2.
//!
//! Sends a MeshEnvelope containing a fake WeatherPacket every 5 seconds.
//! Use flint-debug on a second board to verify packets are received and decoded.
//!
//! ## Heltec V2 pin mapping
//!
//!   SPI2 (SX1276): SCK GPIO5  MISO GPIO19  MOSI GPIO27  CS GPIO18  RST GPIO14  DIO0 GPIO26
//!   I2C0 (OLED):   SDA GPIO4  SCL GPIO15   RST GPIO16

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Delay, Timer};
use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyleBuilder},
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
        master::{Config as SpiConfig, Spi},
        Mode,
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use flint_proto::{encode, FlintPayload, MeshEnvelope, WeatherPacket, MAX_PACKET_BYTES};
use log::{error, info};
use lora_phy::{
    iv::GenericSx127xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx127x::{Config as Sx127xConfig, Sx127x, Sx1276},
    LoRa,
};
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};

esp_bootloader_esp_idf::esp_app_desc!();

// ── LoRa channel config ───────────────────────────────────────────────────────
// Must match flint-debug exactly.

const LORA_FREQUENCY_HZ: u32 = 915_000_000;
const LORA_SF: SpreadingFactor = SpreadingFactor::_7;
const LORA_BW: Bandwidth = Bandwidth::_125KHz;
const LORA_CR: CodingRate = CodingRate::_4_5;
const LORA_PUBLIC_NETWORK: bool = false;
const LORA_TX_POWER_DBM: i32 = 14;

/// Hardcoded node address for this test board.
const NODE_ADDR: u32 = 0x0000_0001;
const NODE_ID: u8 = 1;

/// Interval between transmissions.
const TX_INTERVAL_MS: u64 = 5_000;

// ── Display / formatting helpers ──────────────────────────────────────────────

fn fmt_count(buf: &mut [u8; 16], count: u32) -> &str {
    buf[..4].copy_from_slice(b"TX: ");
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

// ── Entry point ───────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    esp_println::logger::init_logger_from_env();

    info!("flint-node booting");

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

    display.clear(BinaryColor::Off).unwrap();
    Text::with_baseline("FlintNode TX", Point::new(0, 0), text_style, Baseline::Top)
        .draw(&mut display)
        .unwrap();
    Text::with_baseline("TX: 0", Point::new(0, 20), text_style, Baseline::Top)
        .draw(&mut display)
        .unwrap();
    display.flush().unwrap();

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

    let spi_dev = ExclusiveDevice::new(spi, cs, Delay).unwrap();

    // ── SX1276 interface variant ──────────────────────────────────────────────

    let reset = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    let dio0 = Input::new(peripherals.GPIO26, InputConfig::default().with_pull(Pull::Down));

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

    let mut tx_params = lora
        .create_tx_packet_params(8, false, true, false, &mdm)
        .expect("invalid tx packet params");

    let mut sequence: u16 = 0;
    let mut tx_count: u32 = 0;

    info!(
        "TX loop — {}Hz  SF{}  BW{:?}  CR{:?}  {}ms interval",
        LORA_FREQUENCY_HZ, LORA_SF as u8, LORA_BW, LORA_CR, TX_INTERVAL_MS,
    );

    // ── Main transmit loop ────────────────────────────────────────────────────

    loop {
        let payload = FlintPayload::SensorReading(WeatherPacket {
            node_id: NODE_ID,
            temp_c: 2350,       // 23.50 °C
            humidity_pct: 60,
            wind_speed_ms: 0,
            wind_dir_deg: 0,
            fuel_moisture: 50,
            battery_soc: 85,
            battery_mv: 3800,
            sequence,
        });

        let envelope = MeshEnvelope::new_broadcast(NODE_ADDR, sequence as u32, payload);

        let mut tx_buf = [0u8; MAX_PACKET_BYTES];
        match encode(&envelope, &mut tx_buf) {
            Ok(bytes) => {
                match lora.prepare_for_tx(&mdm, &mut tx_params, LORA_TX_POWER_DBM, bytes).await {
                    Ok(_) => match lora.tx().await {
                        Ok(_) => {
                            tx_count = tx_count.saturating_add(1);
                            info!("TX seq={} ({} bytes)", sequence, bytes.len());

                            let mut count_buf = [0u8; 16];
                            let count_str = fmt_count(&mut count_buf, tx_count);
                            display.clear(BinaryColor::Off).unwrap();
                            Text::with_baseline(
                                "FlintNode TX",
                                Point::new(0, 0),
                                text_style,
                                Baseline::Top,
                            )
                            .draw(&mut display)
                            .unwrap();
                            Text::with_baseline(
                                count_str,
                                Point::new(0, 20),
                                text_style,
                                Baseline::Top,
                            )
                            .draw(&mut display)
                            .unwrap();
                            display.flush().unwrap();
                        }
                        Err(e) => error!("tx: {:?}", e),
                    },
                    Err(e) => error!("prepare_for_tx: {:?}", e),
                }
            }
            Err(e) => error!("encode: {:?}", e),
        }

        sequence = sequence.wrapping_add(1);
        Timer::after_millis(TX_INTERVAL_MS).await;
    }
}
