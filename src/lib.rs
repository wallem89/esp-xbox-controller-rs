#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "esp32-c3")]
pub mod esp32;
#[cfg(feature = "esp32-c3")]
pub use esp32::{ControllerConfig, ControllerSelector, run};
pub use xbox_controller_core::*;
