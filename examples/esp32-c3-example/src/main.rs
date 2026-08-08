#![no_std]
#![no_main]

use bt_hci::{cmd::le::LeSetScanParams, controller::ControllerCmdSync};
use embassy_executor::Spawner;
use embassy_futures::{
    join::join,
    select::{Either3, select3},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::{
    interrupt::software::SoftwareInterruptControl,
    rng::{Trng, TrngSource},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;
use xbox_controller_core::parse_input_report;

// Metadata consumed by the ESP-IDF-compatible second-stage bootloader and
// espflash. The application itself remains a bare-metal, no_std esp-hal binary.
esp_bootloader_esp_idf::esp_app_desc!();

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 3;
const GATT_SERVICES_MAX: usize = 16;
const XBOX_NAME: &[u8] = b"Xbox Wireless Controller";
const HID_SERVICE_UUID: [u8; 2] = [0x12, 0x18];

/// Which compatible controller the BLE scan should select.
///
/// `AnyXbox` is appropriate for the example: put only the desired controller
/// in pairing mode. Use `Address` when multiple compatible controllers may be
/// advertising. Bytes are written in the same left-to-right order as the usual
/// `AA:BB:CC:DD:EE:FF` notation.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerSelector {
    AnyXbox,
    Address([u8; 6]),
}

const CONTROLLER_SELECTOR: ControllerSelector = ControllerSelector::AnyXbox;

type Peer = (AddrKind, BdAddr);

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timer_group.timer0, software_interrupts.software_interrupt0);

    // Pairing requires cryptographically secure randomness. Keep the entropy
    // source alive for as long as TrouBLE's security manager uses the TRNG.
    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let mut trng = Trng::try_new().expect("failed to initialize true random generator");

    let connector = BleConnector::new(peripherals.BT, Default::default())
        .expect("failed to initialize the BLE controller");
    let controller: ExternalController<_, 20> = ExternalController::new(connector);

    println!("esp-radio BLE controller initialized");
    run_xbox_central(controller, &mut trng).await
}

async fn run_xbox_central<C>(controller: C, random: &mut Trng) -> !
where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
{
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_generator_seed(random);
    stack.set_io_capabilities(IoCapabilities::NoInputNoOutput);

    let Host {
        mut central,
        mut runner,
        ..
    } = stack.build();
    let found = Signal::<CriticalSectionRawMutex, Peer>::new();
    let finder = XboxAdvertisementFinder { found: &found };

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

            let filter = [(target.0, &target.1)];
            let config = ConnectConfig {
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
            let connection = match central.connect(&config).await {
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

            match select3(
                client.task(),
                use_xbox_reports(&client),
                monitor_connection(&stack, &connection),
            )
            .await
            {
                Either3::First(result) => println!("GATT client stopped: {:?}", result),
                Either3::Second(()) => println!("Xbox report session stopped"),
                Either3::Third(()) => println!("BLE connection event task stopped"),
            }
            connection.disconnect();
            println!("connection ended; rescanning in 1 second");
            embassy_time::Timer::after_secs(1).await;
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

async fn use_xbox_reports<C: Controller, P: PacketPool, const SERVICES: usize>(
    client: &GattClient<'_, C, P, SERVICES>,
) {
    println!("discovering HID service 0x1812");
    let services = match client.services_by_uuid(&Uuid::new_short(0x1812)).await {
        Ok(services) => services,
        Err(error) => {
            println!("HID service discovery failed: {:?}", error);
            return;
        }
    };
    let Some(hid) = services.first() else {
        println!("controller has no HID service");
        return;
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
            return;
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
            return;
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
            return;
        }
    };
    let mut report_map_value = [0_u8; 255];
    match client
        .read_characteristic_long(&report_map, &mut report_map_value)
        .await
    {
        Ok(len) => println!("HID Report Map read: {} bytes", len),
        Err(error) => {
            println!("HID Report Map read failed: {:?}", error);
            return;
        }
    }

    println!("discovering HID Report characteristic 0x2A4D");
    let report: Characteristic<[u8]> = match client
        .characteristic_by_uuid(hid, &Uuid::new_short(0x2a4d))
        .await
    {
        Ok(report) => report,
        Err(error) => {
            println!("Report characteristic discovery failed: {:?}", error);
            return;
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
            return;
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
                return;
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
            return;
        }
    };
    println!("ready; move a stick or press a button");

    loop {
        let notification = notifications.next().await;
        handle_notification(notification.as_ref());
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
}

impl EventHandler for XboxAdvertisementFinder<'_> {
    fn on_adv_reports(&self, mut reports: LeAdvReportsIter<'_>) {
        while let Some(Ok(report)) = reports.next() {
            if advertisement_is_xbox(report.data)
                && selector_matches(CONTROLLER_SELECTOR, report.addr.into_inner())
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
            AdStructure::ServiceUuids16(uuids) => uuids.contains(&HID_SERVICE_UUID),
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

fn handle_notification(bytes: &[u8]) {
    println!("BLE notification ({} bytes): {:02x?}", bytes.len(), bytes);
    match parse_input_report(bytes) {
        Ok(state) => println!("controller state: {:?}", state),
        Err(error) => println!("input report parse error: {:?}", error),
    }
}
