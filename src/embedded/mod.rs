//! # Embedded Microcontroller & Bare-Metal Silicon Module (Tier B)
//!
//! Provides ultra-compact database capabilities tailored for microcontrollers (ESP32,
//! STM32, Cortex-M, RISC-V), SPI NOR Flash storage (W25Q128), and Smart Home edge devices
//! running with `< 512 KB` of SRAM.

pub mod flash;

pub use flash::{
    DEFAULT_MICRO_BLOCK_SIZE, FlashBlockDevice, MICRO_DATABASE_MAGIC, MicroDatabase, MicroHeader,
    MicroPager, RamBlockDevice,
};
