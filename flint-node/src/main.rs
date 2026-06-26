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

use defmt::{error, info, warn};
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
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Timer, with_timeout};
use embedded_hal_bus::spi::ExclusiveDevice;
use flint_proto::{FlintPayload, MAX_PACKET_BYTES, MeshEnvelope, WeatherPacket, encode};
use lora_phy::{
    LoRa,
    iv::GenericSx126xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage},
};

bind_interrupts!(struct Irqs {
    TWISPI0 => twim::InterruptHandler<peripherals::TWISPI0>;
    // Radio SPI on TWISPI1 (SPIM1), matching the lora-phy RAK4631 example. The
    // high-speed SPIM3 instance has clock-domain quirks that gave an intermittent
    // SPI link (GetStatus flapping 0x2a ↔ 0x00); a plain SPIM instance is stable.
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

/// Panic handler that blinks the green status LED (P1.03) forever.
///
/// `panic-probe` only surfaces a panic when a debug probe is attached; on bare
/// USB / external power its fault is a silent infinite loop, which is exactly
/// what made the cold-boot init failures invisible. Raw register writes are used
/// so this works with no HAL state and no probe.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    const P1_BASE: usize = 0x5000_0300;
    const OUTSET: *mut u32 = (P1_BASE + 0x508) as *mut u32;
    const OUTCLR: *mut u32 = (P1_BASE + 0x50C) as *mut u32;
    const DIRSET: *mut u32 = (P1_BASE + 0x518) as *mut u32;
    const LED: u32 = 1 << 3; // P1.03 (RAK19007 green LED)

    cortex_m::interrupt::disable();
    unsafe {
        core::ptr::write_volatile(DIRSET, LED); // drive P1.03 as output
        loop {
            core::ptr::write_volatile(OUTSET, LED);
            for _ in 0..400_000 {
                cortex_m::asm::nop();
            }
            core::ptr::write_volatile(OUTCLR, LED);
            for _ in 0..400_000 {
                cortex_m::asm::nop();
            }
        }
    }
}

/// Blink the status LED `n` times (~120 ms period) for visual diagnostics while
/// the firmware is still alive (sensor retries, fatal-before-reset, etc.).
async fn blink(led: &mut Output<'_>, n: u8) {
    for _ in 0..n {
        led.set_low();
        Timer::after_millis(60).await;
        led.set_high();
        Timer::after_millis(60).await;
    }
}

/// Mailbox: a request to flash the status LED `n` times. Holds only the latest
/// request (fine for a low-rate TX heartbeat); use a Channel if you need a FIFO.
static BLINK: Signal<CriticalSectionRawMutex, u8> = Signal::new();

/// Perpetual task that owns the status LED and flashes it on request, so the TX
/// loop just fires `BLINK.signal(n)` and moves on without paying the flash time.
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) {
    led.set_low();
    loop {
        let n = BLINK.wait().await; // parks here at ~0 CPU until signalled
        for _ in 0..n {
            led.set_high();
            Timer::after_millis(30).await;
            led.set_low();
            Timer::after_millis(70).await;
        }
    }
}

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
async fn main(spawner: Spawner) {
    // Enable the second-stage internal DC/DC (VDD→core) instead of the default
    // LDO — ~25% lower active current. The RAK4630 module has the REG1 inductor.
    // (reg0, the VDDH→VDD high-voltage-mode stage, is left off: it needs the REG0
    // inductor fitted, which isn't confirmed for this board.)
    let mut config = embassy_nrf::config::Config::default();
    config.dcdc.reg1 = true;
    let p = embassy_nrf::init(config);

    // Green status LED (RAK19007 LED1, P1.03). Solid = booting, retry-blink =
    // waiting on a peripheral, flash = each transmission. This is the only boot
    // feedback available when no debug probe is attached.
    let mut led = Output::new(p.P1_03, Level::High, OutputDrive::Standard);

    // General power-rail settle on cold boot — covers the 3V3 rail and sensor
    // power-up before any bus traffic. Generous because a bare USB/battery boot
    // has no debugger latency in front of it (unlike `cargo run`).
    Timer::after_millis(300).await;

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
    let pin_txen  = p.P1_07; // RF switch: enable TX path (RAK4631 RF switch TX)
    let pin_rxen  = p.P1_05; // RF switch: enable RX path (RAK4631 RF switch RX)

    // ── I2C0 for BME680 (RAK1906) ────────────────────────────────────────────

    let mut i2c_buf = [0u8; 256];
    let i2c = Twim::new(p.TWISPI0, Irqs, pin_sda, pin_scl, twim::Config::default(), &mut i2c_buf);
    let mut bme680 = AsyncBme680::new(
        i2c,
        DeviceAddress::Primary, // 0x76 — SDO tied low on RAK1906
        Delay,
        20, // ambient temperature estimate (°C) for gas heater calibration
    );
    // Gas (VOC) measurement disabled to measure baseline power without the
    // 300°C/150ms heater. Re-enable by dropping the `gas_config: None` override
    // (gas is on in `Configuration::default()`).
    let bme_config = Configuration { gas_config: None, ..Default::default() };

    // Retry instead of panicking: on a cold boot the sensor rail may still be
    // settling. A persistent failure degrades to (0,0) readings (handled in the
    // TX loop) rather than bricking the node — the radio is the important part.
    let mut bme_ok = false;
    for attempt in 1..=5u8 {
        match bme680.initialize(&bme_config).await {
            Ok(()) => {
                bme_ok = true;
                break;
            }
            Err(_) => {
                warn!("BME680 init failed (attempt {}/5), retrying", attempt);
                blink(&mut led, 2).await;
                Timer::after_millis(200).await;
            }
        }
    }
    if !bme_ok {
        error!("BME680 init failed — continuing without sensor (check RAK1906 seating)");
    }

    // ── TWISPI1 (SPIM1) for SX1262 ───────────────────────────────────────────
    // Radio is on TWISPI1, not the high-speed SPIM3 instance — see Irqs above.

    let mut spi_cfg = spim::Config::default();
    spi_cfg.frequency = spim::Frequency::M1;

    let spi     = Spim::new(p.TWISPI1, Irqs, pin_sck, pin_miso, pin_mosi, spi_cfg);
    let cs      = Output::new(pin_nss, Level::High, OutputDrive::Standard);
    let spi_dev = ExclusiveDevice::new(spi, cs, Delay).unwrap();

    // ── SX1262 interface variant ──────────────────────────────────────────────

    let reset = Output::new(pin_reset, Level::High, OutputDrive::Standard);
    let dio1  = Input::new(pin_dio1, Pull::Down);
    let busy  = Input::new(pin_busy, Pull::None);
    let txen  = Output::new(pin_txen, Level::Low, OutputDrive::Standard);
    let rxen  = Output::new(pin_rxen, Level::Low, OutputDrive::Standard);

    // Arg order is (rf_switch_rx, rf_switch_tx) — RX first — per the lora-phy API.
    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, Some(rxen), Some(txen))
        .unwrap();

    // ── Radio init ────────────────────────────────────────────────────────────
    // TCXO on SX1262 DIO3 at 1.7 V — the value used by the official lora-phy
    // RAK4631 example. Uses the DCDC regulator.

    let sx_config = Sx126xConfig {
        chip: Sx1262,
        tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
        use_dcdc: true,
        rx_boost: false,
    };

    // If the radio won't init (most likely a cold-boot timing/power transient),
    // blink a visible fatal pattern and do a clean system reset so the whole
    // boot — including the settle delays above — runs again from scratch. This
    // turns a silent permanent hang into a self-healing retry.
    let mut lora = match LoRa::new(Sx126x::new(spi_dev, iv, sx_config), LORA_PUBLIC_NETWORK, Delay)
        .await
    {
        Ok(l) => l,
        Err(e) => {
            error!("LoRa init failed: {:?} — resetting to retry", e);
            blink(&mut led, 10).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
    };

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

    // Hand the LED to its own task; from here we just signal it. Init is done, so
    // we no longer need synchronous LED control on this task.
    // `led_task(led)` returns Result<SpawnToken, SpawnError> (Err = pool slot
    // already in use); unwrap is the idiom embassy's own examples use.
    spawner.spawn(led_task(led).unwrap());

    loop {
        let (temp_c, humidity_pct, gas_ohms) = match bme680.measure().await {
            Ok(m) => {
                let t = (m.temperature * 100.0) as i16;
                let h = m.humidity as u8;
                // gas_resistance is None until the heater plate stabilizes (warm-up)
                // or if gas measurement is disabled; report 0 = no valid reading.
                let gas = m.gas_resistance.map(|r| r as u32).unwrap_or(0);
                info!(
                    "BME680: {}.{:02}°C  {}%RH  gas={} Ω",
                    t / 100, (t % 100).unsigned_abs(), h, gas,
                );
                (t, h, gas)
            }
            Err(_) => {
                error!("BME680 read failed");
                (0, 0, 0)
            }
        };

        let payload = FlintPayload::SensorReading(WeatherPacket {
            node_id: NODE_ID,
            temp_c,
            humidity_pct,
            gas_resistance_ohms: gas_ohms,
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
                    // SetTx is issued with its hardware timeout disabled, and
                    // lora-phy has no software timeout — so a missed TxDone IRQ
                    // (DIO1 never asserts) would hang here forever. Bound it so a
                    // failed TX is logged and the loop continues instead of
                    // freezing on solid-green.
                    Ok(_) => match with_timeout(Duration::from_secs(3), lora.tx()).await {
                        Ok(Ok(_)) => {
                            tx_count = tx_count.saturating_add(1);
                            BLINK.signal(1); // flash once, concurrently — doesn't delay the loop
                            info!("TX seq={} count={} ({} bytes)", sequence, tx_count, bytes.len());
                        }
                        Ok(Err(e)) => error!("tx: {:?}", e),
                        Err(_) => {
                            error!("tx timed out (3s) — no TxDone on DIO1; check TCXO/PLL or DIO1 wiring");
                            BLINK.signal(3); // 3 quick flashes = TX failure
                        }
                    },
                    Err(e) => error!("prepare_for_tx: {:?}", e),
                }
            }
            Err(_) => error!("encode failed — increase MAX_PACKET_BYTES"),
        }

        // Warm-sleep the radio during the idle window — turns off the TCXO and
        // standby draw (~1-2 mA). Warm start retains config in retention RAM, so
        // next cycle's `prepare_for_tx` wakes it (via ensure_ready) without a full
        // re-calibration. The BME680 read at the top of the loop needs no radio.
        if let Err(e) = lora.sleep(true).await {
            warn!("lora.sleep failed: {:?}", e);
        }

        sequence = sequence.wrapping_add(1);
        Timer::after_millis(TX_INTERVAL_MS).await;
    }
}
