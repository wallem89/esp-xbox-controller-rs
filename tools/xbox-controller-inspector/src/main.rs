#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use btleplug::{
    api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use eframe::egui;
use esp_xbox_controller::{XboxButtonState, XboxControllerState, parse_input_report};
use evdev::{
    AbsoluteAxisCode, EventSummary, FFEffectCode, FFEffectData, FFEffectKind, FFReplay, FFTrigger,
    KeyCode,
};
use futures::StreamExt as _;
use tokio::sync::mpsc as tokio_mpsc;
use uuid::{Uuid, uuid};

const REPORT_CHARACTERISTIC: Uuid = uuid!("00002a4d-0000-1000-8000-00805f9b34fb");
const FIRMWARE_CHARACTERISTIC: Uuid = uuid!("00002a26-0000-1000-8000-00805f9b34fb");
const BATTERY_CHARACTERISTIC: Uuid = uuid!("00002a19-0000-1000-8000-00805f9b34fb");

#[derive(Clone, Debug)]
struct Device {
    id: String,
    name: String,
    address: String,
    rssi: Option<i16>,
    likely_xbox: bool,
    state: DeviceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum DeviceState {
    Connected,
    Disconnected,
    NotSetUp,
}

enum Command {
    Scan,
    StopScan,
    Connect(String),
    Read(String),
    Forget(String),
    Vibrate,
    Disconnect(String),
}

enum Event {
    Status(String),
    Scanning(bool),
    Devices(Vec<Device>),
    Connected {
        device_id: String,
        name: String,
        address: String,
        firmware: String,
        battery: Option<u8>,
        can_vibrate: bool,
    },
    Report(Vec<u8>),
    Battery(u8),
    Share(bool),
    LinuxInput {
        state: XboxControllerState,
        event: String,
    },
    Disconnected,
    Error(String),
}

struct InspectorApp {
    command_tx: tokio_mpsc::UnboundedSender<Command>,
    event_rx: Receiver<Event>,
    devices: Vec<Device>,
    selected: Option<String>,
    status: String,
    scanning: bool,
    active_device_id: Option<String>,
    connected_name: Option<String>,
    address: String,
    firmware: String,
    battery: Option<u8>,
    can_vibrate: bool,
    state: XboxControllerState,
    raw_report: Vec<u8>,
    report_error: Option<String>,
    linux_event: String,
}

impl InspectorApp {
    fn new(command_tx: tokio_mpsc::UnboundedSender<Command>, event_rx: Receiver<Event>) -> Self {
        let _ = command_tx.send(Command::Scan);
        Self {
            command_tx,
            event_rx,
            devices: Vec::new(),
            selected: None,
            status: "Starting Bluetooth scan…".into(),
            scanning: false,
            active_device_id: None,
            connected_name: None,
            address: "—".into(),
            firmware: "—".into(),
            battery: None,
            can_vibrate: false,
            state: XboxControllerState::default(),
            raw_report: Vec::new(),
            report_error: None,
            linux_event: "Waiting for Linux input events…".into(),
        }
    }

    fn receive_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                Event::Status(status) => self.status = status,
                Event::Scanning(scanning) => self.scanning = scanning,
                Event::Devices(devices) => self.devices = devices,
                Event::Connected {
                    device_id,
                    name,
                    address,
                    firmware,
                    battery,
                    can_vibrate,
                } => {
                    self.status = format!("Connected to {name}");
                    self.active_device_id = Some(device_id);
                    self.connected_name = Some(name);
                    self.address = address;
                    self.firmware = firmware;
                    self.battery = battery;
                    self.can_vibrate = can_vibrate;
                }
                Event::Report(bytes) => {
                    match parse_input_report(&bytes) {
                        Ok(state) => {
                            self.state = state;
                            self.report_error = None;
                        }
                        Err(error) => self.report_error = Some(format!("{error:?}")),
                    }
                    self.raw_report = bytes;
                }
                Event::Battery(level) => self.battery = Some(level.min(100)),
                Event::Share(pressed) => self.state.buttons.share = pressed,
                Event::LinuxInput { state, event } => {
                    self.state = state;
                    self.linux_event = event;
                    self.report_error = None;
                }
                Event::Disconnected => {
                    self.status = "Controller disconnected".into();
                    self.active_device_id = None;
                    self.connected_name = None;
                    self.can_vibrate = false;
                    self.battery = None;
                }
                Event::Error(error) => self.status = format!("Error: {error}"),
            }
        }
    }
}

impl eframe::App for InspectorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.receive_events();
        ui.ctx().request_repaint_after(Duration::from_millis(50));

        egui::Panel::top("header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Xbox Controller Inspector");
                ui.separator();
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!("v{}", env!("CARGO_PKG_VERSION")));
                });
            });
        });

        egui::Panel::left("devices").min_size(285.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("BLE devices");
                let scan_label = if self.scanning {
                    "Stop scanning"
                } else {
                    "Scan"
                };
                if ui.button(scan_label).clicked() {
                    let command = if self.scanning {
                        Command::StopScan
                    } else {
                        Command::Scan
                    };
                    let _ = self.command_tx.send(command);
                }
            });
            ui.small("Likely Xbox/HID devices are shown first.");
            ui.separator();
            for (heading, state, visible_rows) in [
                ("Paired (connected)", DeviceState::Connected, 5),
                ("Paired (disconnected)", DeviceState::Disconnected, 5),
                ("Not Set Up", DeviceState::NotSetUp, 10),
            ] {
                ui.heading(heading);
                let devices = self
                    .devices
                    .iter()
                    .filter(|device| device.state == state)
                    .cloned()
                    .collect::<Vec<_>>();
                if devices.is_empty() {
                    ui.weak("No devices");
                } else {
                    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 2.0
                        + ui.spacing().item_spacing.y;
                    egui::ScrollArea::vertical()
                        .id_salt(("device_list", state))
                        .max_height(row_height * visible_rows as f32)
                        .show(ui, |ui| {
                            for device in devices {
                                let marker = if device.likely_xbox { "🎮 " } else { "" };
                                let rssi =
                                    device.rssi.map_or(String::new(), |v| format!("  {v} dBm"));
                                let text =
                                    format!("{marker}{}\n{}{}", device.name, device.address, rssi);
                                ui.horizontal(|ui| {
                                    if ui
                                        .selectable_label(
                                            self.selected.as_ref() == Some(&device.id),
                                            text,
                                        )
                                        .clicked()
                                    {
                                        self.selected = Some(device.id.clone());
                                    }
                                    if state == DeviceState::Connected && device.likely_xbox {
                                        let is_reading =
                                            self.active_device_id.as_ref() == Some(&device.id);
                                        if ui
                                            .add_enabled(
                                                !is_reading,
                                                egui::Button::new(if is_reading {
                                                    "Reading"
                                                } else {
                                                    "Read"
                                                }),
                                            )
                                            .clicked()
                                        {
                                            self.selected = Some(device.id.clone());
                                            let _ = self
                                                .command_tx
                                                .send(Command::Read(device.id.clone()));
                                        }
                                    }
                                    let (label, command) = match state {
                                        DeviceState::Connected => {
                                            ("Disconnect", Command::Disconnect(device.id.clone()))
                                        }
                                        DeviceState::Disconnected => {
                                            ("Forget", Command::Forget(device.id.clone()))
                                        }
                                        DeviceState::NotSetUp => {
                                            ("Connect", Command::Connect(device.id.clone()))
                                        }
                                    };
                                    if ui.button(label).clicked() {
                                        self.selected = Some(device.id.clone());
                                        if state == DeviceState::Disconnected {
                                            self.devices.retain(|listed| listed.id != device.id);
                                            self.selected = None;
                                        } else if state == DeviceState::Connected
                                            && let Some(listed) = self
                                                .devices
                                                .iter_mut()
                                                .find(|listed| listed.id == device.id)
                                        {
                                            listed.state = DeviceState::Disconnected;
                                        }
                                        let _ = self.command_tx.send(command);
                                    }
                                });
                            }
                        });
                }
                ui.add_space(10.0);
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(
                self.connected_name
                    .as_deref()
                    .unwrap_or("No controller connected"),
            );
            egui::Grid::new("identity").striped(true).show(ui, |ui| {
                ui.label("BLE address");
                ui.monospace(&self.address);
                ui.end_row();
                ui.label("Firmware");
                ui.monospace(&self.firmware);
                ui.end_row();
                ui.label("Battery");
                ui.label(
                    self.battery
                        .map(|level| format!("🔋 {level}%"))
                        .unwrap_or_else(|| "—".into()),
                );
                ui.end_row();
            });
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    self.can_vibrate,
                    egui::Button::new("Test vibration (0.2 s)"),
                )
                .clicked()
            {
                let _ = self.command_tx.send(Command::Vibrate);
            }
            if self.connected_name.is_some() && !self.can_vibrate {
                ui.small("No BLE output report or Linux force-feedback device was discovered.");
            }
            ui.separator();
            ui.columns(2, |columns| {
                columns[0].heading("Buttons");
                button_grid(&mut columns[0], self.state.buttons);
                columns[1].heading("Analog inputs");
                analog_ui(&mut columns[1], self.state);
            });
            ui.separator();
            ui.heading("Raw HID input report");
            let raw = if self.raw_report.is_empty() {
                "Not exposed by BlueZ; using Linux input events below.".into()
            } else {
                self.raw_report
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            ui.monospace(raw);
            if let Some(error) = &self.report_error {
                ui.colored_label(egui::Color32::YELLOW, format!("Parser: {error}"));
            }
            ui.add_space(6.0);
            ui.label("Last Linux input event");
            ui.monospace(&self.linux_event);
        });
    }
}

fn button_grid(ui: &mut egui::Ui, buttons: XboxButtonState) {
    let entries = [
        ("A", buttons.a),
        ("B", buttons.b),
        ("X", buttons.x),
        ("Y", buttons.y),
        ("D-pad ↑", buttons.dpad_up),
        ("D-pad ↓", buttons.dpad_down),
        ("D-pad ←", buttons.dpad_left),
        ("D-pad →", buttons.dpad_right),
        ("LB", buttons.lb),
        ("RB", buttons.rb),
        ("Left stick", buttons.left_stick_button),
        ("Right stick", buttons.right_stick_button),
        ("View", buttons.view),
        ("Menu", buttons.menu),
        ("Xbox", buttons.xbox),
        ("Share", buttons.share),
    ];
    egui::Grid::new("buttons").num_columns(2).show(ui, |ui| {
        for (index, (name, pressed)) in entries.into_iter().enumerate() {
            let color = if pressed {
                egui::Color32::LIGHT_GREEN
            } else {
                ui.visuals().weak_text_color()
            };
            ui.colored_label(
                color,
                if pressed {
                    format!("● {name}")
                } else {
                    format!("○ {name}")
                },
            );
            if index % 2 == 1 {
                ui.end_row();
            }
        }
    });
}

fn analog_ui(ui: &mut egui::Ui, state: XboxControllerState) {
    egui::Grid::new("analog").striped(true).show(ui, |ui| {
        for (name, value) in [
            ("Left X", state.sticks.left_x),
            ("Left Y", state.sticks.left_y),
            ("Right X", state.sticks.right_x),
            ("Right Y", state.sticks.right_y),
        ] {
            ui.label(name);
            ui.monospace(format!("{value:6}"));
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.label(format!("Left trigger: {}", state.triggers.left));
    ui.add(egui::ProgressBar::new(
        f32::from(state.triggers.left) / 1023.0,
    ));
    ui.label(format!("Right trigger: {}", state.triggers.right));
    ui.add(egui::ProgressBar::new(
        f32::from(state.triggers.right) / 1023.0,
    ));
}

fn find_and_start_evdev_reader(
    address: &str,
    bluetooth_name: &str,
    event_tx: Sender<Event>,
    input_generation: Arc<AtomicU64>,
    generation: u64,
) -> Option<std::path::PathBuf> {
    let normalized_address = address.replace(':', "").to_ascii_lowercase();
    let wanted_name = bluetooth_name.to_ascii_lowercase();
    let mut selected = None;
    for _ in 0..20 {
        let mut gamepads = evdev::enumerate()
            .filter(|(_, device)| {
                device
                    .supported_keys()
                    .is_some_and(|keys| keys.contains(KeyCode::BTN_SOUTH))
            })
            .collect::<Vec<_>>();

        if let Some(index) = gamepads.iter().position(|(_, device)| {
            device
                .unique_name()
                .unwrap_or_default()
                .replace(':', "")
                .eq_ignore_ascii_case(&normalized_address)
        }) {
            selected = Some(gamepads.swap_remove(index));
        } else {
            // A name is only a safe fallback when it identifies exactly one
            // gamepad. Multiple Xbox controllers usually have identical names.
            gamepads.retain(|(_, device)| {
                let name = device.name().unwrap_or_default().to_ascii_lowercase();
                name.contains("xbox")
                    || name.contains("x-box")
                    || (!wanted_name.is_empty() && name == wanted_name)
            });
            if gamepads.len() == 1 {
                selected = gamepads.pop();
            }
        }
        if selected.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let (path, mut device) = selected?;
    start_share_button_readers(
        &path,
        &normalized_address,
        event_tx.clone(),
        input_generation.clone(),
        generation,
    );
    let axis_ranges = device
        .get_absinfo()
        .ok()?
        .map(|(code, info)| (code, (info.minimum(), info.maximum())))
        .collect::<HashMap<_, _>>();
    let xbox_ble_layout = axis_ranges.contains_key(&AbsoluteAxisCode::ABS_BRAKE)
        && axis_ranges.contains_key(&AbsoluteAxisCode::ABS_GAS);
    let reader_path = path.clone();
    thread::spawn(move || {
        let mut state = XboxControllerState::default();
        loop {
            if input_generation.load(Ordering::Relaxed) != generation {
                break;
            }
            match device.fetch_events() {
                Ok(events) => {
                    for input in events {
                        if input_generation.load(Ordering::Relaxed) != generation {
                            return;
                        }
                        let summary = input.destructure();
                        let event = format!("{summary:?}");
                        apply_linux_input(&mut state, summary, &axis_ranges, xbox_ble_layout);
                        event_tx.send(Event::LinuxInput { state, event }).ok();
                    }
                }
                Err(error) => {
                    event_tx
                        .send(Event::Error(format!(
                            "reading {}: {error}",
                            reader_path.display()
                        )))
                        .ok();
                    break;
                }
            }
        }
    });
    Some(path)
}

fn start_evdev_watcher(
    address: String,
    bluetooth_name: String,
    event_tx: Sender<Event>,
    input_generation: Arc<AtomicU64>,
    generation: u64,
) {
    thread::spawn(move || {
        // BlueZ may establish the BLE link before the kernel has finished creating
        // the HID input node. Keep looking instead of giving up after the initial
        // short lookup in `find_and_start_evdev_reader`.
        for _ in 0..12 {
            if input_generation.load(Ordering::Relaxed) != generation {
                return;
            }
            if find_and_start_evdev_reader(
                &address,
                &bluetooth_name,
                event_tx.clone(),
                input_generation.clone(),
                generation,
            )
            .is_some()
            {
                event_tx
                    .send(Event::Status(
                        "Connected; receiving Linux controller input".into(),
                    ))
                    .ok();
                return;
            }
            thread::sleep(Duration::from_millis(500));
        }
        event_tx
            .send(Event::Error(
                "Linux did not expose a readable controller input device within 30 seconds; check pairing and /dev/input permissions"
                    .into(),
            ))
            .ok();
    });
}

fn apply_linux_input(
    state: &mut XboxControllerState,
    event: EventSummary,
    axis_ranges: &HashMap<AbsoluteAxisCode, (i32, i32)>,
    xbox_ble_layout: bool,
) {
    match event {
        EventSummary::Key(_, code, value) => {
            let pressed = value != 0;
            match code {
                KeyCode::BTN_SOUTH => state.buttons.a = pressed,
                KeyCode::BTN_EAST => state.buttons.b = pressed,
                KeyCode::BTN_NORTH => state.buttons.x = pressed,
                KeyCode::BTN_WEST => state.buttons.y = pressed,
                KeyCode::BTN_TL => state.buttons.lb = pressed,
                KeyCode::BTN_TR => state.buttons.rb = pressed,
                KeyCode::BTN_SELECT => state.buttons.view = pressed,
                KeyCode::BTN_START => state.buttons.menu = pressed,
                KeyCode::BTN_MODE => state.buttons.xbox = pressed,
                KeyCode::BTN_THUMBL => state.buttons.left_stick_button = pressed,
                KeyCode::BTN_THUMBR => state.buttons.right_stick_button = pressed,
                KeyCode::BTN_DPAD_UP => state.buttons.dpad_up = pressed,
                KeyCode::BTN_DPAD_DOWN => state.buttons.dpad_down = pressed,
                KeyCode::BTN_DPAD_LEFT => state.buttons.dpad_left = pressed,
                KeyCode::BTN_DPAD_RIGHT => state.buttons.dpad_right = pressed,
                KeyCode::KEY_RECORD | KeyCode::KEY_F12 => state.buttons.share = pressed,
                _ => {}
            }
        }
        EventSummary::AbsoluteAxis(_, code, value) => {
            let range = axis_ranges.get(&code).copied().unwrap_or((0, 65_535));
            match code {
                AbsoluteAxisCode::ABS_X => state.sticks.left_x = normalize_stick(value, range),
                AbsoluteAxisCode::ABS_Y => state.sticks.left_y = normalize_stick(value, range),
                AbsoluteAxisCode::ABS_Z if xbox_ble_layout => {
                    state.sticks.right_x = normalize_stick(value, range)
                }
                AbsoluteAxisCode::ABS_RZ if xbox_ble_layout => {
                    state.sticks.right_y = normalize_stick(value, range)
                }
                AbsoluteAxisCode::ABS_RX if !xbox_ble_layout => {
                    state.sticks.right_x = normalize_stick(value, range)
                }
                AbsoluteAxisCode::ABS_RY if !xbox_ble_layout => {
                    state.sticks.right_y = normalize_stick(value, range)
                }
                AbsoluteAxisCode::ABS_Z => state.triggers.left = normalize_trigger(value, range),
                AbsoluteAxisCode::ABS_RZ => state.triggers.right = normalize_trigger(value, range),
                AbsoluteAxisCode::ABS_BRAKE => {
                    state.triggers.left = normalize_trigger(value, range)
                }
                AbsoluteAxisCode::ABS_GAS => state.triggers.right = normalize_trigger(value, range),
                AbsoluteAxisCode::ABS_HAT0X => {
                    state.buttons.dpad_left = value < 0;
                    state.buttons.dpad_right = value > 0;
                }
                AbsoluteAxisCode::ABS_HAT0Y => {
                    state.buttons.dpad_up = value < 0;
                    state.buttons.dpad_down = value > 0;
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn start_share_button_readers(
    gamepad_path: &std::path::Path,
    normalized_address: &str,
    event_tx: Sender<Event>,
    input_generation: Arc<AtomicU64>,
    generation: u64,
) {
    for (path, mut device) in evdev::enumerate() {
        if path == gamepad_path {
            continue;
        }
        let keys = device.supported_keys();
        let exposes_share = keys.is_some_and(|keys| {
            keys.contains(KeyCode::KEY_RECORD) || keys.contains(KeyCode::KEY_F12)
        });
        let unique = device
            .unique_name()
            .unwrap_or_default()
            .replace(':', "")
            .to_ascii_lowercase();
        let belongs_to_controller = !unique.is_empty() && unique == normalized_address;
        if !exposes_share || !belongs_to_controller {
            continue;
        }
        let tx = event_tx.clone();
        let input_generation = input_generation.clone();
        thread::spawn(move || {
            while input_generation.load(Ordering::Relaxed) == generation {
                let Ok(events) = device.fetch_events() else {
                    break;
                };
                for input in events {
                    if input_generation.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    if let EventSummary::Key(_, KeyCode::KEY_RECORD | KeyCode::KEY_F12, value) =
                        input.destructure()
                    {
                        tx.send(Event::Share(value != 0)).ok();
                    }
                }
            }
        });
    }
}

fn normalize_stick(value: i32, (minimum, maximum): (i32, i32)) -> i16 {
    if maximum <= minimum {
        return 0;
    }
    let position = i64::from(value.clamp(minimum, maximum) - minimum);
    let span = i64::from(maximum - minimum);
    ((position * 65_535 / span) - 32_768).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

fn normalize_trigger(value: i32, (minimum, maximum): (i32, i32)) -> u16 {
    if maximum <= minimum {
        return 0;
    }
    let position = i64::from(value.clamp(minimum, maximum) - minimum);
    let span = i64::from(maximum - minimum);
    (position * 1023 / span).clamp(0, 1023) as u16
}

fn evdev_supports_rumble(path: &std::path::Path) -> bool {
    evdev::Device::open(path).is_ok_and(|device| {
        device
            .supported_ff()
            .is_some_and(|effects| effects.contains(FFEffectCode::FF_RUMBLE))
    })
}

async fn pair_xbox_controller(address: &str) -> Result<()> {
    let address = address
        .parse::<bluer::Address>()
        .context("parsing the controller Bluetooth address")?;
    let session = bluer::Session::new().await?;
    let _agent = session
        .register_agent(bluer::agent::Agent::default())
        .await
        .context("registering the automatic BlueZ pairing agent")?;
    let adapter = session.default_adapter().await?;
    let device = adapter.device(address)?;
    if !device.is_paired().await? {
        device.pair().await.context("pairing the controller")?;
    }
    device.set_trusted(true).await?;
    Ok(())
}

struct Backend {
    adapter: Adapter,
    event_tx: Sender<Event>,
    devices: HashMap<String, Peripheral>,
    connected: Option<Peripheral>,
    output: Option<btleplug::api::Characteristic>,
    evdev_path: Option<std::path::PathBuf>,
    input_generation: Arc<AtomicU64>,
    scanning: bool,
}

impl Backend {
    async fn new(event_tx: Sender<Event>) -> Result<Self> {
        let manager = Manager::new().await.context("initializing BlueZ")?;
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no Bluetooth adapter found"))?;
        Ok(Self {
            adapter,
            event_tx,
            devices: HashMap::new(),
            connected: None,
            output: None,
            evdev_path: None,
            input_generation: Arc::new(AtomicU64::new(0)),
            scanning: false,
        })
    }

    async fn scan(&mut self) -> Result<()> {
        if self.scanning {
            self.event_tx
                .send(Event::Status("Bluetooth scan is already running".into()))
                .ok();
            return Ok(());
        }
        self.event_tx
            .send(Event::Status("Scanning for BLE devices…".into()))
            .ok();
        self.adapter.start_scan(ScanFilter::default()).await?;
        self.scanning = true;
        self.event_tx.send(Event::Scanning(true)).ok();
        tokio::time::sleep(Duration::from_secs(3)).await;
        self.refresh_devices().await?;
        self.event_tx
            .send(Event::Status(
                "Scanning… device list updates automatically".into(),
            ))
            .ok();
        Ok(())
    }

    async fn refresh_devices(&mut self) -> Result<()> {
        let pairing_adapter = match bluer::Session::new().await {
            Ok(session) => session.default_adapter().await.ok(),
            Err(_) => None,
        };
        let mut found = Vec::new();
        self.devices.clear();
        for peripheral in self.adapter.peripherals().await? {
            let id = peripheral.id().to_string();
            let Some(properties) = peripheral.properties().await? else {
                continue;
            };
            let address = properties.address.to_string();
            let advertised_name = properties
                .local_name
                .or(properties.advertisement_name)
                .filter(|name| !name.is_empty() && name != &address);
            let (bluez_name, paired) = if let (Some(adapter), Ok(address)) =
                (&pairing_adapter, address.parse::<bluer::Address>())
            {
                match adapter.device(address) {
                    Ok(device) => (
                        device
                            .alias()
                            .await
                            .ok()
                            .filter(|name| !name.is_empty() && name != &address.to_string())
                            .or(device.name().await.ok().flatten()),
                        device.is_paired().await.unwrap_or(false),
                    ),
                    Err(_) => (None, false),
                }
            } else {
                (None, false)
            };
            let name = bluez_name
                .or(advertised_name)
                .unwrap_or_else(|| "Unknown device".into());
            let likely_xbox = name.to_ascii_lowercase().contains("xbox")
                || properties.manufacturer_data.contains_key(&0x045e);
            let connected = peripheral.is_connected().await.unwrap_or(false);
            let state = if connected {
                DeviceState::Connected
            } else if paired {
                DeviceState::Disconnected
            } else {
                DeviceState::NotSetUp
            };
            found.push(Device {
                id: id.clone(),
                name,
                address,
                rssi: properties.rssi,
                likely_xbox,
                state,
            });
            self.devices.insert(id, peripheral);
        }
        found.sort_by_key(|device| {
            (
                !device.likely_xbox,
                device.state,
                device.name.to_ascii_lowercase(),
            )
        });
        self.event_tx.send(Event::Devices(found)).ok();
        Ok(())
    }

    async fn stop_scan(&mut self) -> Result<()> {
        if self.scanning {
            self.adapter.stop_scan().await?;
            self.scanning = false;
            self.event_tx.send(Event::Scanning(false)).ok();
            self.event_tx
                .send(Event::Status("Bluetooth scan stopped".into()))
                .ok();
        }
        Ok(())
    }

    async fn rescan(&mut self) -> Result<()> {
        self.stop_scan().await?;
        self.scan().await
    }

    async fn activate(&mut self, id: &str, pair: bool) -> Result<()> {
        self.stop_scan().await?;
        let generation = self.input_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let peripheral = self
            .devices
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("selected device is no longer available; scan again"))?;
        let advertised = peripheral
            .properties()
            .await?
            .ok_or_else(|| anyhow!("device properties unavailable"))?;
        let advertised_name = advertised
            .local_name
            .as_deref()
            .or(advertised.advertisement_name.as_deref())
            .unwrap_or_default();
        let bluez_name = if let Ok(session) = bluer::Session::new().await {
            if let (Ok(adapter), Ok(address)) = (
                session.default_adapter().await,
                advertised.address.to_string().parse::<bluer::Address>(),
            ) {
                match adapter.device(address) {
                    Ok(device) => device.alias().await.unwrap_or_default(),
                    Err(_) => String::new(),
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        let is_xbox = advertised_name.to_ascii_lowercase().contains("xbox")
            || bluez_name.to_ascii_lowercase().contains("xbox")
            || advertised.manufacturer_data.contains_key(&0x045e);
        if !is_xbox {
            return Err(anyhow!(
                "automatic pairing is limited to devices identified as Xbox controllers"
            ));
        }
        if pair {
            self.event_tx
                .send(Event::Status(
                    "Pairing controller and automatically accepting confirmation…".into(),
                ))
                .ok();
            pair_xbox_controller(&advertised.address.to_string()).await?;
            self.event_tx.send(Event::Status("Connecting…".into())).ok();
        } else {
            self.event_tx
                .send(Event::Status("Starting controller input reader…".into()))
                .ok();
        }
        if !peripheral.is_connected().await? {
            peripheral
                .connect_with_timeout(Duration::from_secs(15))
                .await?;
        }
        peripheral
            .discover_services_with_timeout(Duration::from_secs(15))
            .await?;

        let characteristics = peripheral.characteristics();
        let inputs = characteristics
            .iter()
            .filter(|characteristic| {
                characteristic.uuid == REPORT_CHARACTERISTIC
                    && characteristic
                        .properties
                        .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
            })
            .cloned()
            .collect::<Vec<_>>();
        self.output = characteristics
            .iter()
            .find(|characteristic| {
                characteristic.uuid == REPORT_CHARACTERISTIC
                    && characteristic
                        .properties
                        .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
            })
            .cloned();

        let firmware = if let Some(characteristic) = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == FIRMWARE_CHARACTERISTIC)
        {
            peripheral
                .read(characteristic)
                .await
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Unavailable".into())
        } else {
            "Not exposed".into()
        };
        let battery_characteristic = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == BATTERY_CHARACTERISTIC)
            .cloned();
        let battery = if let Some(characteristic) = &battery_characteristic {
            peripheral
                .read(characteristic)
                .await
                .ok()
                .and_then(|bytes| bytes.first().copied())
                .map(|level| level.min(100))
        } else {
            None
        };

        for input in &inputs {
            if input.properties.contains(CharPropFlags::READ)
                && let Ok(bytes) = peripheral.read(input).await
                && !bytes.is_empty()
            {
                self.event_tx.send(Event::Report(bytes)).ok();
            }
        }
        let mut subscribed = Vec::new();
        for input in &inputs {
            if peripheral.subscribe(input).await.is_ok() {
                subscribed.push((input.uuid, input.service_uuid));
            }
        }
        let battery_subscribed = if let Some(characteristic) = &battery_characteristic {
            characteristic
                .properties
                .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
                && peripheral.subscribe(characteristic).await.is_ok()
        } else {
            false
        };
        if !subscribed.is_empty() || battery_subscribed {
            let mut notifications = peripheral.notifications().await?;
            let tx = self.event_tx.clone();
            let input_generation = self.input_generation.clone();
            tokio::spawn(async move {
                while let Some(notification) = notifications.next().await {
                    if input_generation.load(Ordering::Relaxed) != generation {
                        break;
                    }
                    if notification.uuid == BATTERY_CHARACTERISTIC {
                        if let Some(level) = notification.value.first() {
                            tx.send(Event::Battery(*level)).ok();
                        }
                    } else if subscribed.contains(&(notification.uuid, notification.service_uuid))
                        && notification.value.len() >= esp_xbox_controller::INPUT_REPORT_LEN
                    {
                        tx.send(Event::Report(notification.value)).ok();
                    }
                }
            });
        }

        let properties = peripheral
            .properties()
            .await?
            .ok_or_else(|| anyhow!("device properties unavailable"))?;
        let name = properties
            .local_name
            .or(properties.advertisement_name)
            .unwrap_or_else(|| "Xbox controller".into());
        self.evdev_path = find_and_start_evdev_reader(
            &properties.address.to_string(),
            &name,
            self.event_tx.clone(),
            self.input_generation.clone(),
            generation,
        );
        if self.evdev_path.is_none() {
            start_evdev_watcher(
                properties.address.to_string(),
                name.clone(),
                self.event_tx.clone(),
                self.input_generation.clone(),
                generation,
            );
        }
        let can_vibrate = self.output.is_some()
            || self
                .evdev_path
                .as_ref()
                .is_some_and(|path| evdev_supports_rumble(path));
        let waiting_for_linux_input = self.evdev_path.is_none() && inputs.is_empty();
        self.event_tx
            .send(Event::Connected {
                device_id: id.to_owned(),
                name,
                address: properties.address.to_string(),
                firmware,
                battery,
                can_vibrate,
            })
            .ok();
        if waiting_for_linux_input {
            self.event_tx
                .send(Event::Status(
                    "BLE connected; waiting for Linux to expose the controller input device…"
                        .into(),
                ))
                .ok();
        }
        self.connected = Some(peripheral);
        Ok(())
    }

    async fn connect(&mut self, id: &str) -> Result<()> {
        self.activate(id, true).await
    }

    async fn read(&mut self, id: &str) -> Result<()> {
        self.activate(id, false).await
    }

    async fn vibrate(&self) -> Result<()> {
        if let (Some(peripheral), Some(output)) = (&self.connected, &self.output) {
            const PULSE: [u8; 8] = [0x0f, 50, 50, 50, 50, 20, 0, 0];
            let write_type = if output
                .properties
                .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
            {
                WriteType::WithoutResponse
            } else {
                WriteType::WithResponse
            };
            peripheral.write(output, &PULSE, write_type).await?;
        } else if let Some(path) = &self.evdev_path {
            let mut device = evdev::Device::open(path)
                .with_context(|| format!("opening {} for force feedback", path.display()))?;
            let mut effect = device.upload_ff_effect(FFEffectData {
                direction: 0,
                trigger: FFTrigger::default(),
                replay: FFReplay {
                    length: 200,
                    delay: 0,
                },
                kind: FFEffectKind::Rumble {
                    strong_magnitude: u16::MAX / 2,
                    weak_magnitude: u16::MAX / 2,
                },
            })?;
            effect.play(1)?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            effect.stop()?;
        } else {
            return Err(anyhow!("no force-feedback controller device is available"));
        }
        self.event_tx
            .send(Event::Status("Vibration command sent".into()))
            .ok();
        Ok(())
    }

    async fn disconnect(&mut self, id: &str) -> Result<()> {
        let peripheral = self
            .devices
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("selected device is no longer available; scan again"))?;
        let is_active = self
            .connected
            .as_ref()
            .is_some_and(|connected| connected.id() == peripheral.id());
        peripheral.disconnect().await?;
        if is_active {
            self.input_generation.fetch_add(1, Ordering::Relaxed);
            self.connected = None;
            self.output = None;
            self.evdev_path = None;
            self.event_tx.send(Event::Disconnected).ok();
        } else {
            self.event_tx
                .send(Event::Status("Controller disconnected".into()))
                .ok();
        }
        Ok(())
    }

    async fn forget(&mut self, id: &str) -> Result<()> {
        let peripheral = self
            .devices
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("selected device is no longer available; scan again"))?;
        let properties = peripheral
            .properties()
            .await?
            .ok_or_else(|| anyhow!("device properties unavailable"))?;
        let address = properties
            .address
            .to_string()
            .parse::<bluer::Address>()
            .context("parsing the controller Bluetooth address")?;
        let session = bluer::Session::new().await?;
        session
            .default_adapter()
            .await?
            .remove_device(address)
            .await?;
        self.devices.remove(id);
        self.event_tx
            .send(Event::Status(
                "Controller pairing removed; rescanning…".into(),
            ))
            .ok();
        self.rescan().await
    }
}

fn start_backend() -> (tokio_mpsc::UnboundedSender<Command>, Receiver<Event>) {
    let (command_tx, mut command_rx) = tokio_mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                event_tx
                    .send(Event::Error(format!("starting async runtime: {error}")))
                    .ok();
                return;
            }
        };
        runtime.block_on(async move {
            let mut backend = match Backend::new(event_tx.clone()).await {
                Ok(backend) => backend,
                Err(error) => {
                    event_tx.send(Event::Error(format!("{error:#}"))).ok();
                    return;
                }
            };
            let mut refresh_interval = tokio::time::interval(Duration::from_secs(2));
            refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let result = tokio::select! {
                    command = command_rx.recv() => {
                        let Some(command) = command else { break };
                        match command {
                            Command::Scan => backend.scan().await,
                            Command::StopScan => backend.stop_scan().await,
                            Command::Connect(id) => {
                                let connect_result = backend.connect(&id).await;
                                let rescan_result = backend.rescan().await;
                                connect_result.and(rescan_result)
                            }
                            Command::Read(id) => backend.read(&id).await,
                            Command::Forget(id) => backend.forget(&id).await,
                            Command::Vibrate => backend.vibrate().await,
                            Command::Disconnect(id) => backend.disconnect(&id).await,
                        }
                    }
                    _ = refresh_interval.tick(), if backend.scanning => {
                        backend.refresh_devices().await
                    }
                };
                if let Err(error) = result {
                    event_tx.send(Event::Error(format!("{error:#}"))).ok();
                }
            }
        });
    });
    (command_tx, event_rx)
}

fn configure_system_font(context: &egui::Context) {
    const FONT_PATHS: [&str; 2] = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    ];
    let Some(bytes) = FONT_PATHS.iter().find_map(|path| std::fs::read(path).ok()) else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    let name = "ubuntu_unicode_fallback".to_owned();
    fonts.font_data.insert(
        name.clone(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(fonts) = fonts.families.get_mut(&family) {
            fonts.push(name.clone());
        }
    }
    context.set_fonts(fonts);
}

fn main() -> eframe::Result {
    let (command_tx, event_rx) = start_backend();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 900.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Xbox Controller Inspector",
        options,
        Box::new(move |creation_context| {
            configure_system_font(&creation_context.egui_ctx);
            Ok(Box::new(InspectorApp::new(command_tx, event_rx)))
        }),
    )
}
