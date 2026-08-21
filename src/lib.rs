#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod error;
mod report;
mod state;

#[cfg(feature = "esp32-c3")]
pub mod esp32;
#[cfg(feature = "esp32-c3")]
pub use esp32::{ControllerSelector, run};

pub use error::ParseError;
pub use report::{INPUT_REPORT_LEN, parse_input_report};
pub use state::{XboxButtonState, XboxControllerState, XboxSticks, XboxTriggers};
