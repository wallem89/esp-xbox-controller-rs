#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Duration;
use esp_backtrace as _;
use esp_hal::{
    peripherals::BT,
    rng::{Trng, TrngSource},
    timer::timg::TimerGroup,
};
use esp_radio::ble::controller::BleConnector;
use esp_xbox_controller::{ControllerConfig, ControllerSelector, XboxControllerState, run};
use log::{error, info, warn};
use trouble_host::prelude::ExternalController;

esp_bootloader_esp_idf::esp_app_desc!();

const CONTROLLER_ADDRESS: [u8; 6] = [0xAC, 0x8E, 0xBD, 0x4D, 0xAF, 0x5A]; // BLE address of xbox controller
const IDLE_DISCONNECT_TIME: Duration = Duration::from_secs(5 * 60); // After this time inactivity the controller will be disconnected
const CONTROLLER: ControllerConfig = ControllerConfig::new(
    ControllerSelector::Address(CONTROLLER_ADDRESS),
    Some(IDLE_DISCONNECT_TIME),
);
const CHANNEL_CAPACITY: usize = 8;

static CONTROLLER_STATES: Channel<CriticalSectionRawMutex, XboxControllerState, CHANNEL_CAPACITY> =
    Channel::new();

#[embassy_executor::task]
async fn controller_task(bt: BT<'static>, _entropy_source: TrngSource<'static>) {
    let mut random = Trng::try_new().expect("failed to initialize true random generator");
    let connector =
        BleConnector::new(bt, Default::default()).expect("failed to initialize BLE controller");
    let ble = ExternalController::<_, 20>::new(connector);
    let mut previous = None;

    run(
        ble,
        &mut random,
        CONTROLLER,
        |notification| match notification {
            Ok(state) if previous != Some(state) => {
                previous = Some(state);
                if CONTROLLER_STATES.try_send(state).is_err() {
                    warn!("controller state channel full; dropping update");
                }
            }
            Ok(_) => {}
            Err(error) => error!("input report parse error: {:?}", error),
        },
    )
    .await
}

#[embassy_executor::task]
async fn print_task() {
    loop {
        let state = CONTROLLER_STATES.receive().await;
        info!("controller state: {:?}", state);
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    info!(
        "Initializing device with esp32-c3-task-example firmware version {}...",
        env!("CARGO_PKG_VERSION")
    );
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timer_group.timer0, peripherals.FROM_CPU_INTR0);

    let entropy_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    spawner.spawn(print_task().unwrap());
    spawner.spawn(controller_task(peripherals.BT, entropy_source).unwrap());

    core::future::pending().await
}
