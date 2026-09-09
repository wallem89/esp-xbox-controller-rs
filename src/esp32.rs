use bt_hci::{cmd::le::LeSetScanParams, controller::ControllerCmdSync};
use embassy_futures::{
    join::join,
    select::{Either, Either3, select, select3},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::rng::Trng;
use esp_println::println;
use trouble_host::prelude::*;

use crate::{ParseError, XboxControllerState, parse_input_report};

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 3;
const GATT_SERVICES_MAX: usize = 16;
const XBOX_NAME: &[u8] = b"Xbox Wireless Controller";
const HID_SERVICE_UUID: [u8; 2] = [0x12, 0x18];
const HID_IDLE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const IDLE_DISCONNECT_COOLDOWN: Duration = Duration::from_secs(30);
// Select all four actuators but command zero power. A zero selection mask is
// ignored by Xbox firmware and therefore does not count as host activity.
const IDLE_KEEPALIVE_REPORT: [u8; 8] = [0x0f, 0, 0, 0, 0, 0, 0, 0];

/// Which compatible controller the BLE scan should select.
///
/// `AnyXbox` is appropriate for the example: put only the desired controller
/// in pairing mode. Use `Address` when multiple compatible controllers may be
/// advertising. Bytes are written in the same left-to-right order as the usual
/// `AA:BB:CC:DD:EE:FF` notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSelector {
    AnyXbox,
    Address([u8; 6]),
}

/// Connection behavior for an Xbox controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerConfig {
    /// Which compatible advertising controller may be connected.
    pub selector: ControllerSelector,
    /// Disconnect after this much time without a changed input report.
    /// `None` keeps the controller awake indefinitely.
    pub idle_disconnect_after: Option<Duration>,
}

impl ControllerConfig {
    /// Creates a configuration that keeps the selected controller connected.
    pub const fn new(
        selector: ControllerSelector,
        idle_disconnect_after: Option<Duration>,
    ) -> Self {
        Self {
            selector,
            idle_disconnect_after,
        }
    }
}

type Peer = (AddrKind, BdAddr);

/// Connects to the selected controller, reconnecting when necessary, and calls
/// `on_notification` for every input report received from it.
pub async fn run<C, F>(
    controller: C,
    random: &mut Trng,
    config: ControllerConfig,
    mut on_notification: F,
) -> !
where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
    F: FnMut(Result<XboxControllerState, ParseError>),
{
    let mut resources: HostResources<C, DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_generator_seed(random)
        .set_io_capabilities(IoCapabilities::NoInputNoOutput)
        .build();
    let mut central = stack.central();
    let mut runner = stack.runner();
    let found = Signal::<CriticalSectionRawMutex, Peer>::new();
    let finder = XboxAdvertisementFinder {
        found: &found,
        selector: config.selector,
    };

    join(runner.run_with_handler(&finder), async {
        let mut attempt: u32 = 0;
        loop {
            println!("scanning for Xbox Wireless Controller (HID 0x1812)");
            let (returned_central, target) = scan_for_xbox(central, &found).await;
            central = returned_central;
            let Some(target) = target else {
                println!("scan failed; retrying in 1 second");
                embassy_time::Timer::after_secs(1).await;
                continue;
            };
            let target_address = Address {
                kind: target.0,
                addr: target.1,
            };
            attempt = attempt.wrapping_add(1);
            println!("connection attempt {} to {}", attempt, target_address);

            // Let the controller and host controller settle after active scan
            // cancellation. Model 1708 is unreliable when LE Create Connection
            // immediately follows its advertisement/scan response.
            Timer::after_millis(100).await;

            let filter = [target_address];
            let connection_config = ConnectConfig {
                // NimBLE-compatible initial parameters also work better with
                // older BLE 4.x Xbox controllers such as Model 1708 than
                // TrouBLE 0.6's relatively slow fixed 80 ms default.
                connect_params: RequestedConnParams {
                    min_connection_interval: Duration::from_millis(15),
                    max_connection_interval: Duration::from_millis(30),
                    max_latency: 0,
                    min_event_length: Duration::from_secs(0),
                    max_event_length: Duration::from_secs(0),
                    supervision_timeout: Duration::from_secs(5),
                },
                scan_config: ScanConfig {
                    filter_accept_list: &filter,
                    ..Default::default()
                },
            };
            let connection = match central.connect(&connection_config).await {
                Ok(connection) => connection,
                Err(error) => {
                    println!("BLE connection failed: {:?}; rescanning", error);
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                }
            };
            println!("connected; starting pairing/encryption");

            // Give the first connection event time to complete before sending
            // SMP traffic. Debug logging previously supplied this delay by
            // accident; keeping it explicit makes release behavior stable.
            Timer::after_millis(100).await;

            // Do not request a persistent bond until this example also stores
            // the matching key in ESP32 flash. A controller-only bond becomes
            // stale whenever the board resets and loses TrouBLE's RAM state.
            if let Err(error) = connection.set_bondable(false) {
                println!(
                    "could not configure non-bondable pairing: {:?}; reconnecting",
                    error
                );
                connection.disconnect();
                embassy_time::Timer::after_secs(1).await;
                continue;
            }
            if let Err(error) = connection.request_security() {
                println!("could not request link security: {:?}; reconnecting", error);
                connection.disconnect();
                embassy_time::Timer::after_secs(1).await;
                continue;
            }
            match with_timeout(
                Duration::from_secs(20),
                wait_for_pairing(&stack, &connection),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                }
                Err(_) => {
                    println!("application pairing deadline reached after 20000 ms");
                    connection.disconnect();
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                }
            }

            println!("creating GATT client");
            let client = match GattClient::<C, DefaultPacketPool, GATT_SERVICES_MAX>::new(
                &stack,
                &connection,
            )
            .await
            {
                Ok(client) => client,
                Err(error) => {
                    println!("could not create GATT client: {:?}; reconnecting", error);
                    connection.disconnect();
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                }
            };

            let idle_disconnect = match select3(
                client.task(),
                use_xbox_reports(&client, &mut on_notification, config.idle_disconnect_after),
                monitor_connection(&stack, &connection),
            )
            .await
            {
                Either3::First(result) => {
                    println!("GATT client stopped: {:?}", result);
                    false
                }
                Either3::Second(idle_disconnect) => {
                    if idle_disconnect {
                        println!("controller idle timeout reached; disconnecting");
                    } else {
                        println!("Xbox report session stopped");
                    }
                    idle_disconnect
                }
                Either3::Third(()) => {
                    println!("BLE connection event task stopped");
                    false
                }
            };
            connection.disconnect();
            if idle_disconnect {
                println!("waiting for controller shutdown before rescanning");
                Timer::after(IDLE_DISCONNECT_COOLDOWN).await;
            } else {
                println!("connection ended; rescanning in 1 second");
                Timer::after_secs(1).await;
            }
        }
    })
    .await;

    unreachable!()
}

async fn monitor_connection<C: Controller, P: PacketPool>(
    stack: &Stack<'_, C, P>,
    connection: &Connection<'_, P>,
) {
    loop {
        match connection.next().await {
            ConnectionEvent::RequestConnectionParams(request) => {
                println!("controller requested connection parameter update");
                match request.accept(None, stack).await {
                    Ok(()) => println!("connection parameter update accepted"),
                    Err(error) => {
                        println!("connection parameter update failed: {:?}", error);
                        return;
                    }
                }
            }
            ConnectionEvent::Disconnected { reason } => {
                println!("controller disconnected: {:?}", reason);
                return;
            }
            ConnectionEvent::PairingComplete { security_level, .. } => {
                println!("security changed: {:?}", security_level)
            }
            ConnectionEvent::PairingFailed(error) => {
                println!("pairing/security update failed: {:?}", error);
                return;
            }
            ConnectionEvent::PassKeyDisplay(passkey) => {
                println!("pairing passkey: {}", passkey)
            }
            ConnectionEvent::PassKeyConfirm(passkey) => {
                println!("confirming pairing passkey: {}", passkey);
                if let Err(error) = connection.pass_key_confirm() {
                    println!("passkey confirmation failed: {:?}", error);
                    return;
                }
            }
            ConnectionEvent::PassKeyInput => {
                println!("controller requested unsupported passkey input");
                return;
            }
            event => println!("post-pairing connection event: {:?}", event),
        }
    }
}

async fn use_xbox_reports<C: Controller, P: PacketPool, const SERVICES: usize, F>(
    client: &GattClient<'_, C, P, SERVICES>,
    on_notification: &mut F,
    idle_disconnect_after: Option<Duration>,
) -> bool
where
    F: FnMut(Result<XboxControllerState, ParseError>),
{
    println!("discovering HID service 0x1812");
    let services = match client.services_by_uuid(&Uuid::new_short(0x1812)).await {
        Ok(services) => services,
        Err(error) => {
            println!("HID service discovery failed: {:?}", error);
            return false;
        }
    };
    let Some(hid) = services.first() else {
        println!("controller has no HID service");
        return false;
    };

    // Perform the normal HID-over-GATT enumeration reads before subscribing.
    // The working NimBLE reference reads every readable HID characteristic;
    // these two reads, plus the input Report read below, reproduce that setup.
    println!("reading HID Information 0x2A4A");
    let hid_information: Characteristic<[u8]> = match client
        .characteristic_by_uuid(hid, &Uuid::new_short(0x2a4a))
        .await
    {
        Ok(characteristic) => characteristic,
        Err(error) => {
            println!("HID Information discovery failed: {:?}", error);
            return false;
        }
    };
    let mut hid_information_value = [0_u8; 8];
    match client
        .read_characteristic(&hid_information, &mut hid_information_value)
        .await
    {
        Ok(len) => println!(
            "HID Information ({} bytes): {:02x?}",
            len,
            &hid_information_value[..len]
        ),
        Err(error) => {
            println!("HID Information read failed: {:?}", error);
            return false;
        }
    }

    println!("reading HID Report Map 0x2A4B");
    let report_map: Characteristic<[u8]> = match client
        .characteristic_by_uuid(hid, &Uuid::new_short(0x2a4b))
        .await
    {
        Ok(characteristic) => characteristic,
        Err(error) => {
            println!("HID Report Map discovery failed: {:?}", error);
            return false;
        }
    };
    let mut report_map_value = [0_u8; 255];
    match client
        .read_characteristic(&report_map, &mut report_map_value)
        .await
    {
        Ok(len) => println!("HID Report Map read: {} bytes", len),
        Err(error) => {
            println!("HID Report Map read failed: {:?}", error);
            return false;
        }
    }

    initialize_hid_host(client, hid).await;

    println!("discovering HID Report characteristic 0x2A4D");
    let report: Characteristic<[u8]> = match client
        .characteristic_by_uuid(hid, &Uuid::new_short(0x2a4d))
        .await
    {
        Ok(report) => report,
        Err(error) => {
            println!("Report characteristic discovery failed: {:?}", error);
            return false;
        }
    };

    // Xbox controllers require the Report value to be read before the CCCD
    // subscription is enabled. Some firmwares return an empty first read, so
    // retry that case once.
    println!(
        "selected Report handle 0x{:04x}, CCCD {:?}",
        report.handle, report.cccd_handle
    );
    let mut initial_report = [0_u8; 64];
    let mut initial_len = match client
        .read_characteristic(&report, &mut initial_report)
        .await
    {
        Ok(len) => len,
        Err(error) => {
            println!("initial Report read failed: {:?}", error);
            return false;
        }
    };
    if initial_len == 0 {
        initial_len = match client
            .read_characteristic(&report, &mut initial_report)
            .await
        {
            Ok(len) => len,
            Err(error) => {
                println!("second initial Report read failed: {:?}", error);
                return false;
            }
        };
    }
    println!(
        "initial Report value ({} bytes): {:02x?}",
        initial_len,
        &initial_report[..initial_len]
    );

    println!("subscribing to input notifications");
    let mut notifications = match client.subscribe(&report, false).await {
        Ok(notifications) => notifications,
        Err(error) => {
            println!("input notification subscription failed: {:?}", error);
            return false;
        }
    };

    // Keep the discovered table alive for the session because its writable
    // Report characteristic is also used for idle keepalives.
    let hid_characteristics = client.characteristics::<16>(hid).await.ok();
    let output_report = hid_characteristics.as_ref().and_then(|characteristics| {
        characteristics
            .iter()
            .find(|characteristic| characteristic.props.any(&[CharacteristicProp::Write]))
    });
    play_connection_rumble(client, output_report).await;

    println!("ready; move a stick or press a button");
    match idle_disconnect_after {
        Some(duration) => println!(
            "idle disconnect configured for {} seconds",
            duration.as_secs()
        ),
        None => println!("idle disconnect disabled"),
    }
    let mut previous_state = None;
    let mut idle_deadline = idle_disconnect_after.map(|duration| Instant::now() + duration);
    let mut keepalive_deadline = Instant::now() + HID_IDLE_KEEPALIVE_INTERVAL;

    loop {
        let next_timer = idle_deadline
            .map(|idle| idle.min(keepalive_deadline))
            .unwrap_or(keepalive_deadline);
        match select(Timer::at(next_timer), notifications.next()).await {
            Either::First(()) => {
                let now = Instant::now();
                if idle_deadline.is_some_and(|deadline| now >= deadline) {
                    return true;
                }
                if let Some(output_report) = output_report
                    && let Err(error) = client
                        .write_characteristic_without_response(
                            output_report,
                            &IDLE_KEEPALIVE_REPORT,
                        )
                        .await
                {
                    println!("WARNING: controller keepalive write failed: {:?}", error);
                    return false;
                }
                keepalive_deadline = Instant::now() + HID_IDLE_KEEPALIVE_INTERVAL;
            }
            Either::Second(notification) => {
                let parsed = parse_input_report(notification.as_ref());
                if parsed
                    .as_ref()
                    .is_ok_and(|state| Some(*state) != previous_state)
                {
                    previous_state = parsed.as_ref().ok().copied();
                    idle_deadline = idle_disconnect_after.map(|duration| Instant::now() + duration);
                }
                on_notification(parsed);
            }
        }
    }
}

async fn initialize_hid_host<C: Controller, P: PacketPool, const SERVICES: usize>(
    client: &GattClient<'_, C, P, SERVICES>,
    hid: &ServiceHandle,
) {
    // A HID host uses Report Protocol and tells the device it is not suspended.
    // BlueZ performs the same HOG initialization for the desktop inspector.
    if let Ok(protocol_mode) = client
        .characteristic_by_uuid::<u8>(hid, &Uuid::new_short(0x2a4e))
        .await
    {
        let mut mode = [0_u8; 1];
        if client
            .read_characteristic(&protocol_mode, &mut mode)
            .await
            .is_ok()
            && mode[0] != 0x01
        {
            let _ = client
                .write_characteristic_without_response(&protocol_mode, &[0x01])
                .await;
        }
    }

    if let Ok(control_point) = client
        .characteristic_by_uuid::<u8>(hid, &Uuid::new_short(0x2a4c))
        .await
    {
        // HID Control Point value 0x01 means Exit Suspend.
        let _ = client
            .write_characteristic_without_response(&control_point, &[0x01])
            .await;
    }
}

async fn play_connection_rumble<C: Controller, P: PacketPool, const SERVICES: usize>(
    client: &GattClient<'_, C, P, SERVICES>,
    output_report: Option<&Characteristic<[u8]>>,
) {
    let Some(output_report) = output_report else {
        println!("WARNING: controller has no writable HID output Report");
        return;
    };

    // Xbox PID output Report ID 3 payload (the Report ID is represented by the
    // GATT characteristic and is not included in the value): select all four
    // motors, 50% power, active for 0.20 seconds, no repeat.
    const CONNECTED_PULSE: [u8; 8] = [0x0f, 50, 50, 50, 50, 20, 0, 0];
    println!(
        "playing connection rumble via handle 0x{:04x}",
        output_report.handle
    );
    match client
        .write_characteristic_without_response(output_report, &CONNECTED_PULSE)
        .await
    {
        Ok(()) => {}
        Err(error) => {
            println!("WARNING: connection rumble write failed: {:?}", error);
        }
    }
}

async fn scan_for_xbox<'stack, C, P>(
    central: Central<'stack, C, P>,
    found: &Signal<CriticalSectionRawMutex, Peer>,
) -> (Central<'stack, C, P>, Option<Peer>)
where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
    P: PacketPool,
{
    // Discard a late advertisement from the previous scan/connection cycle so
    // reconnect attempts never use a stale peer notification.
    found.reset();
    let mut scanner = Scanner::new(central);
    let scan_config = ScanConfig {
        active: true,
        interval: Duration::from_millis(100),
        window: Duration::from_millis(100),
        ..Default::default()
    };
    let scan_result = scanner.scan(&scan_config).await;
    if let Err(error) = &scan_result {
        println!("BLE scan failed: {:?}", error);
    }
    if scan_result.is_err() {
        core::mem::drop(scan_result);
        return (scanner.into_inner(), None);
    }
    let session = scan_result.unwrap();
    let peer = found.wait().await;
    core::mem::drop(session);
    (scanner.into_inner(), Some(peer))
}

async fn wait_for_pairing<C: Controller, P: PacketPool>(
    stack: &Stack<'_, C, P>,
    connection: &Connection<'_, P>,
) -> bool {
    loop {
        match connection.next().await {
            ConnectionEvent::Encrypted { security_level }
                if matches!(
                    security_level,
                    SecurityLevel::Encrypted | SecurityLevel::EncryptedAuthenticated
                ) =>
            {
                // This connection is non-bondable: HID access needs encryption,
                // not completion of SMP key distribution. Start consuming GATT
                // traffic now so early reports cannot fill its receive queue
                // while we wait for a later PairingComplete event.
                println!("link ready for GATT: {:?}", security_level);
                return true;
            }
            ConnectionEvent::PairingComplete {
                security_level,
                bond,
            } => {
                println!("pairing complete: {:?}", security_level);
                if bond.is_some() {
                    println!("unexpected bond returned during non-bondable pairing");
                }
                return true;
            }
            ConnectionEvent::PairingFailed(error) => {
                println!("pairing failed: {:?}; rescanning", error);
                connection.disconnect();
                return false;
            }
            ConnectionEvent::Disconnected { reason } => {
                println!(
                    "controller disconnected during pairing: {:?}; rescanning",
                    reason
                );
                return false;
            }
            ConnectionEvent::RequestConnectionParams(request) => {
                if let Err(error) = request.accept(None, stack).await {
                    println!(
                        "connection parameter update failed: {:?}; reconnecting",
                        error
                    );
                    connection.disconnect();
                    return false;
                }
            }
            ConnectionEvent::PassKeyDisplay(passkey) => {
                println!("pairing passkey: {}", passkey)
            }
            ConnectionEvent::PassKeyConfirm(passkey) => {
                println!("confirming pairing passkey: {}", passkey);
                if let Err(error) = connection.pass_key_confirm() {
                    println!("passkey confirmation failed: {:?}; reconnecting", error);
                    connection.disconnect();
                    return false;
                }
            }
            ConnectionEvent::PassKeyInput => {
                println!("controller requested unsupported passkey input; reconnecting");
                connection.disconnect();
                return false;
            }
            event => println!("pairing connection event: {:?}", event),
        }
    }
}

struct XboxAdvertisementFinder<'a> {
    found: &'a Signal<CriticalSectionRawMutex, Peer>,
    selector: ControllerSelector,
}

impl EventHandler for XboxAdvertisementFinder<'_> {
    fn on_adv_reports(&self, mut reports: LeAdvReportsIter<'_>) {
        while let Some(Ok(report)) = reports.next() {
            if advertisement_is_xbox(report.data)
                && selector_matches(self.selector, report.addr.into_inner())
            {
                let address = Address {
                    kind: report.addr_kind,
                    addr: report.addr,
                };
                println!(
                    "found compatible Xbox at {} (RSSI {} dBm)",
                    address, report.rssi
                );
                self.found.signal((report.addr_kind, report.addr));
            }
        }
    }
}

fn advertisement_is_xbox(data: &[u8]) -> bool {
    AdStructure::decode(data)
        .flatten()
        .any(|field| match field {
            AdStructure::CompleteLocalName(name) | AdStructure::ShortenedLocalName(name) => {
                name == XBOX_NAME
            }
            AdStructure::IncompleteServiceUuids16(uuids)
            | AdStructure::CompleteServiceUuids16(uuids) => uuids.contains(&HID_SERVICE_UUID),
            _ => false,
        })
}

fn selector_matches(selector: ControllerSelector, raw_address: [u8; 6]) -> bool {
    match selector {
        ControllerSelector::AnyXbox => true,
        ControllerSelector::Address(expected) => {
            let mut displayed_address = raw_address;
            displayed_address.reverse();
            displayed_address == expected
        }
    }
}
