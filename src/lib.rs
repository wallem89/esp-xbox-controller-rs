#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "esp32c3")]
pub mod esp32;
#[cfg(feature = "esp32c3")]
pub use esp32::{ControllerConfig, ControllerEvent, ControllerSelector, run, run_with_events};
pub use xbox_controller_core::*;

#[cfg(any(feature = "esp32c3", test))]
mod idle;
