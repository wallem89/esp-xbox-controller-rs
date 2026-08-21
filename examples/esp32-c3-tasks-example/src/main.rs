#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_backtrace as _;
use esp_hal::{
    interrupt::software::SoftwareInterruptControl,
    peripherals::BT,
    rng::{Trng, TrngSource},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::ble::controller::BleConnector;
use esp_xbox_controller::{ControllerSelector, XboxControllerState, run};
use trouble_host::prelude::ExternalController;

esp_bootloader_esp_idf::esp_app_desc!();

const CONTROLLER: ControllerSelector =
    ControllerSelector::Address([0xC0, 0xD6, 0xD5, 0xEA, 0xBE, 0x85]);
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
                    println!("controller state channel full; dropping update");
                }
            }
            Ok(_) => {}
            Err(error) => println!("input report parse error: {:?}", error),
        },
    )
    .await
}

#[embassy_executor::task]
async fn print_task() {
    loop {
        let state = CONTROLLER_STATES.receive().await;
        println!("controller state: {:?}", state);
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timer_group.timer0, software_interrupts.software_interrupt0);

    let entropy_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    spawner.spawn(print_task().unwrap());
    spawner.spawn(controller_task(peripherals.BT, entropy_source).unwrap());

    core::future::pending().await
}
