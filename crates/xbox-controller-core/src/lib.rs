#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod error;
mod report;
mod state;

pub use error::ParseError;
pub use report::{INPUT_REPORT_LEN, parse_input_report};
pub use state::{XboxButtonState, XboxControllerState, XboxSticks, XboxTriggers};
