use anyhow::Result;
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serde::Deserialize;
use slint::{Model, VecModel};
use std::{rc::Rc, sync::mpsc, time::Duration};

slint::include_modules!();

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

fn main() -> Result<()> {
    let ui = MainWindow::new()?;
    let nodes_model: Rc<VecModel<NodeData>> = Rc::new(VecModel::default());
    ui.set_nodes(nodes_model.clone().into());

    let (tx, rx) = mpsc::channel::<UiMsg>();

    let mqtt_host = std::env::var("FLINT_MQTT_HOST").unwrap_or_else(|_| "localhost".into());
    let mqtt_port: u16 = std::env::var("FLINT_MQTT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1883);
    let topic_prefix =
        std::env::var("FLINT_MQTT_TOPIC_PREFIX").unwrap_or_else(|_| "flintmesh".into());

    std::thread::spawn(move || run_mqtt(mqtt_host, mqtt_port, topic_prefix, tx));

    // Poll the channel on a Slint timer — runs on the main thread so Rc access is safe.
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        {
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
        },
    );

    ui.run()?;
    Ok(())
}

fn run_mqtt(host: String, port: u16, topic_prefix: String, tx: mpsc::Sender<UiMsg>) {
    let mut opts = MqttOptions::new("flint-ui", host, port);
    opts.set_keep_alive(Duration::from_secs(30));

    let (client, mut connection) = Client::new(opts, 64);
    let subscribe_topic = format!("{topic_prefix}/node/+/weather");

    // Initial subscribe — broker will confirm via ConnAck/SubAck.
    client.subscribe(&subscribe_topic, QoS::AtLeastOnce).ok();

    for event in connection.iter() {
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                // Re-subscribe after reconnect.
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
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{:02}:{:02}:{:02} UTC", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}
