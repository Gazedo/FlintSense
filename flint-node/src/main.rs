//! flint-node — LoRa transmit firmware for RAK4631 (nRF52840 + SX1262).
//!
//! Sends a MeshEnvelope containing a fake WeatherPacket every 5 seconds.
//! Flash and monitor logs with `cargo run` via probe-rs over SWD (J10 on RAK19007).
//!
//! ## RAK4631 pin mapping
//!
//!   SPI3 (SX1262):  SCK  P1.11  MISO P1.13  MOSI P1.12  NSS  P1.10
//!   SX1262 control: RST  P1.06  DIO1 P1.15  BUSY P1.14
//!   SX1262 RF sw:   TXEN P1.07  RXEN P1.08
//!
//! All pin assignments are declared as named variables at the top of `main`
//! so they are easy to remap for other boards.

#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use bosch_bme680::{AsyncBme680, Configuration, DeviceAddress};
use embassy_nrf::{
    bind_interrupts,
    gpio::{Input, Level, Output, OutputDrive, Pull},
    peripherals,
    spim::{self, Spim},
    twim::{self, Twim},
};
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use flint_proto::{FlintPayload, MAX_PACKET_BYTES, MeshEnvelope, WeatherPacket, encode};
use lora_phy::{
    LoRa,
    iv::GenericSx126xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage},
};
use panic_probe as _;

bind_interrupts!(struct Irqs {
    TWISPI0 => twim::InterruptHandler<peripherals::TWISPI0>;
    SPIM3 => spim::InterruptHandler<peripherals::SPI3>;
});

// ── LoRa channel config ───────────────────────────────────────────────────────
// Must match the gateway / debug receiver exactly.

const LORA_FREQUENCY_HZ: u32 = 915_000_000;
const LORA_SF: SpreadingFactor = SpreadingFactor::_7;
const LORA_BW: Bandwidth = Bandwidth::_125KHz;
const LORA_CR: CodingRate = CodingRate::_4_5;
const LORA_PUBLIC_NETWORK: bool = false;
const LORA_TX_POWER_DBM: i32 = 14;

/// Hardcoded node identity for this test board.
const NODE_ADDR: u32 = 0x0000_0001;
const NODE_ID: u8 = 1;

/// Interval between transmissions.
const TX_INTERVAL_MS: u64 = 5_000;

// ── Entry point ───────────────────────────────────────────────────────────────

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

    info!("flint-node booting");

    // ── Pin assignments (RAK4631 internal SX1262 connections) ────────────────
    // All RAK4631↔SX1262 connections are internal to the module.
    // To retarget to another board, change these lines only.

    // ── I2C pin assignments (WisBlock sensor slot) ───────────────────────────
    let pin_sda = p.P0_13;
    let pin_scl = p.P0_14;

    let pin_sck   = p.P1_11; // SPI3 clock
    let pin_mosi  = p.P1_12; // SPI3 MOSI
    let pin_miso  = p.P1_13; // SPI3 MISO
    let pin_nss   = p.P1_10; // SX1262 chip select (active low)
    let pin_reset = p.P1_06; // SX1262 hardware reset (active low)
    let pin_dio1  = p.P1_15; // SX1262 DIO1 — TX done / RX done interrupt
    let pin_busy  = p.P1_14; // SX1262 BUSY — high while radio is processing
    let pin_txen  = p.P1_07; // RF switch: enable TX path
    let pin_rxen  = p.P1_08; // RF switch: enable RX path

    // ── I2C0 for BME680 (RAK1906) ────────────────────────────────────────────

    let mut i2c_buf = [0u8; 256];
    let i2c = Twim::new(p.TWISPI0, Irqs, pin_sda, pin_scl, twim::Config::default(), &mut i2c_buf);
    let mut bme680 = AsyncBme680::new(
        i2c,
        DeviceAddress::Primary, // 0x76 — SDO tied low on RAK1906
        Delay,
        20, // ambient temperature estimate (°C) for gas heater calibration
    );
    bme680
        .initialize(&Configuration::default())
        .await
        .expect("BME680 init failed — check RAK1906 seating on WisBlock slot");

    // ── SPI3 for SX1262 ──────────────────────────────────────────────────────

    let mut spi_cfg = spim::Config::default();
    spi_cfg.frequency = spim::Frequency::M1;

    let spi     = Spim::new(p.SPI3, Irqs, pin_sck, pin_miso, pin_mosi, spi_cfg);
    let cs      = Output::new(pin_nss, Level::High, OutputDrive::Standard);
    let spi_dev = ExclusiveDevice::new(spi, cs, Delay).unwrap();

    // ── SX1262 interface variant ──────────────────────────────────────────────

    let reset = Output::new(pin_reset, Level::High, OutputDrive::Standard);
    let dio1  = Input::new(pin_dio1, Pull::Down);
    let busy  = Input::new(pin_busy, Pull::None);
    let txen  = Output::new(pin_txen, Level::Low, OutputDrive::Standard);
    let rxen  = Output::new(pin_rxen, Level::Low, OutputDrive::Standard);

    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, Some(txen), Some(rxen))
        .unwrap();

    // ── Radio init ────────────────────────────────────────────────────────────
    // RAK4631 uses a 32 MHz TCXO at 1.7 V and a DCDC regulator.

    let sx_config = Sx126xConfig {
        chip: Sx1262,
        tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
        use_dcdc: true,
        rx_boost: false,
    };

    let mut lora = LoRa::new(Sx126x::new(spi_dev, iv, sx_config), LORA_PUBLIC_NETWORK, Delay)
        .await
        .expect("LoRa init failed");

    let mdm = lora
        .create_modulation_params(LORA_SF, LORA_BW, LORA_CR, LORA_FREQUENCY_HZ)
        .expect("invalid modulation params");

    let mut tx_params = lora
        .create_tx_packet_params(8, false, true, false, &mdm)
        .expect("invalid tx packet params");

    let mut sequence: u16 = 0;
    let mut tx_count: u32 = 0;

    info!(
        "TX loop — {} Hz  SF{}  {}ms interval",
        LORA_FREQUENCY_HZ,
        LORA_SF as u8,
        TX_INTERVAL_MS,
    );

    // ── Main transmit loop ────────────────────────────────────────────────────

    loop {
        let (temp_c, humidity_pct) = match bme680.measure().await {
            Ok(m) => {
                let t = (m.temperature * 100.0) as i16;
                let h = m.humidity as u8;
                info!("BME680: {}.{:02}°C  {}%RH", t / 100, (t % 100).unsigned_abs(), h);
                (t, h)
            }
            Err(_) => {
                error!("BME680 read failed");
                (0, 0)
            }
        };

        let payload = FlintPayload::SensorReading(WeatherPacket {
            node_id: NODE_ID,
            temp_c,
            humidity_pct,
            wind_speed_ms: 0,
            wind_dir_deg: 0,
            fuel_moisture: 50,
            battery_soc: 85,
            battery_mv: 3800,
            solar_ma: 0,
            load_ma: 0,
            power_flags: 0,
            sequence,
        });

        let envelope = MeshEnvelope::new_broadcast(NODE_ADDR, sequence as u32, payload);
        let mut tx_buf = [0u8; MAX_PACKET_BYTES];

        match encode(&envelope, &mut tx_buf) {
            Ok(bytes) => {
                match lora
                    .prepare_for_tx(&mdm, &mut tx_params, LORA_TX_POWER_DBM, bytes)
                    .await
                {
                    Ok(_) => match lora.tx().await {
                        Ok(_) => {
                            tx_count = tx_count.saturating_add(1);
                            info!("TX seq={} count={} ({} bytes)", sequence, tx_count, bytes.len());
                        }
                        Err(e) => error!("tx: {:?}", e),
                    },
                    Err(e) => error!("prepare_for_tx: {:?}", e),
                }
            }
            Err(_) => error!("encode failed — increase MAX_PACKET_BYTES"),
        }

        sequence = sequence.wrapping_add(1);
        Timer::after_millis(TX_INTERVAL_MS).await;
    }
}
