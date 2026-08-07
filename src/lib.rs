#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod error;
mod report;
mod state;

pub use error::ParseError;
pub use report::parse_input_report;
pub use state::{XboxButtonState, XboxControllerState, XboxSticks, XboxTriggers};
