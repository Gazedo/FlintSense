use anyhow::{Context, Result};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serde::Deserialize;
use slint::{Model, Timer, TimerMode, VecModel};
use std::{
    fs,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

slint::include_modules!();

#[derive(Deserialize)]
struct Config {
    mqtt_host: String,
    #[serde(default = "default_port")]
    mqtt_port: u16,
    #[serde(default = "default_topic_prefix")]
    topic_prefix: String,
}

fn default_port() -> u16 {
    1883
}
fn default_topic_prefix() -> String {
    "flintmesh".into()
}

#[derive(Debug, Deserialize)]
struct MqttNodeData {
    node_id: u8,
    temp_c: f64,
    humidity_pct: u8,
    wind_speed_ms: f64,
    wind_dir_deg: u16,
    fuel_moisture: u8,
    battery_soc: u8,
    battery_mv: u16,
}

enum UiMsg {
    Connected,
    Disconnected,
    NodeUpdate(NodeData),
}

/*
 * Should have multiple views, home page will show the current temperature outside with a small
 * forecast and if rain is expected today along with any weather alerts. The next pages will show
 * the data from the various nodes in a scrollable table? Or it could add a tab for each detected node?
 * Tab for longer forecast. Accesed by clicking on the forecast.
 * Small screen so limit the data on the screen.
 * All touch screen compatible.
 * Timer to go back to main
 * Icon to access display settings, light vs dark and brightness, sleep time out.
 *
 *
 * Main screen will show:
 *   - Current temp
 *   - high/low for day
 *   - Rain or not
 *   - Weather warnings
 *   - AQI
 *   - Weather tomorrow
 * Froecast
 *   - Forecast for 7 days:
 *   - temperature for each day
 *   - Rain or snwo
 * Raw node Data:
 *   - Temp, batt level, wind, humidity will be larger
 *
 */

fn main() -> Result<()> {
    let config: Config = serde_json::from_str(
        &fs::read_to_string("config.json").context("could not read config.json")?,
    )
    .context("invalid config.json")?;

    let ui = MainWindow::new()?;
    let nodes_model: Rc<VecModel<NodeData>> = Rc::new(VecModel::default());

    #[cfg(debug_assertions)]
    {
        nodes_model.push(NodeData {
            node_id: 1,
            temp_c: 28.5,
            humidity_pct: 42,
            wind_speed_ms: 3.5,
            wind_dir_deg: 270,
            fuel_moisture: 12,
            battery_soc: 87,
            battery_mv: 3920,
            last_seen: "14:32:05 UTC".into(),
        });
        nodes_model.push(NodeData {
            node_id: 2,
            temp_c: 31.0,
            humidity_pct: 38,
            wind_speed_ms: 5.0,
            wind_dir_deg: 245,
            fuel_moisture: 9,
            battery_soc: 23,
            battery_mv: 3610,
            last_seen: "14:32:11 UTC".into(),
        });
    }

    ui.set_nodes(nodes_model.clone().into());

    let (tx, rx) = mpsc::channel::<UiMsg>();

    thread::spawn(move || run_mqtt(config.mqtt_host, config.mqtt_port, config.topic_prefix, tx));

    // Poll the channel on a Slint timer — runs on the main thread so Rc access is safe.
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(100), {
        let ui_weak = ui.as_weak();
        let nodes = nodes_model.clone();
        move || {
            while let Ok(msg) = rx.try_recv() {
                let Some(ui) = ui_weak.upgrade() else { return };
                match msg {
                    UiMsg::Connected => ui.set_mqtt_status("Connected".into()),
                    UiMsg::Disconnected => ui.set_mqtt_status("Disconnected".into()),
                    UiMsg::NodeUpdate(data) => {
                        let id = data.node_id;
                        let pos = (0..nodes.row_count())
                            .find(|&i| nodes.row_data(i).is_some_and(|n| n.node_id == id));
                        match pos {
                            Some(i) => nodes.set_row_data(i, data),
                            None => nodes.push(data),
                        }
                    }
                }
            }
        }
    });

    ui.run()?;
    Ok(())
}

fn run_mqtt(host: String, port: u16, topic_prefix: String, tx: mpsc::Sender<UiMsg>) {
    let mut opts = MqttOptions::new("flint-ui", host, port);
    opts.set_keep_alive(Duration::from_secs(30));

    let (client, mut connection) = Client::new(opts, 64);
    let subscribe_topic = format!("{topic_prefix}/node/+/weather");

    client.subscribe(&subscribe_topic, QoS::AtLeastOnce).ok();

    for event in connection.iter() {
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                client.subscribe(&subscribe_topic, QoS::AtLeastOnce).ok();
                tx.send(UiMsg::Connected).ok();
            }
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let Ok(d) = serde_json::from_slice::<MqttNodeData>(&p.payload) else {
                    continue;
                };
                let node = NodeData {
                    node_id: d.node_id as i32,
                    temp_c: d.temp_c as f32,
                    humidity_pct: d.humidity_pct as i32,
                    wind_speed_ms: d.wind_speed_ms as f32,
                    wind_dir_deg: d.wind_dir_deg as i32,
                    fuel_moisture: d.fuel_moisture as i32,
                    battery_soc: d.battery_soc as i32,
                    battery_mv: d.battery_mv as i32,
                    last_seen: now_hms().into(),
                };
                tx.send(UiMsg::NodeUpdate(node)).ok();
            }
            Err(_) => {
                tx.send(UiMsg::Disconnected).ok();
            }
            _ => {}
        }
    }
}

fn now_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{:02}:{:02}:{:02} UTC",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}
