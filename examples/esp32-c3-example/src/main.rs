#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{
    rng::{Trng, TrngSource},
    timer::timg::TimerGroup,
};
use esp_radio::ble::controller::BleConnector;
use esp_xbox_controller::{ControllerConfig, ControllerSelector, run};
use log::{info, warn};
use trouble_host::prelude::ExternalController;

esp_bootloader_esp_idf::esp_app_desc!();

const CONTROLLER: ControllerConfig = ControllerConfig::new(ControllerSelector::AnyXbox, None);

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timer_group.timer0, peripherals.FROM_CPU_INTR0);

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let mut random = Trng::try_new().expect("failed to initialize true random generator");
    let connector = BleConnector::new(peripherals.BT, Default::default())
        .expect("failed to initialize the BLE controller");
    let ble = ExternalController::<_, 20>::new(connector);

    run(
        ble,
        &mut random,
        CONTROLLER,
        |notification| match notification {
            Ok(state) => info!("controller state: {:?}", state),
            Err(error) => warn!("input report parse error: {:?}", error),
        },
    )
    .await
}
