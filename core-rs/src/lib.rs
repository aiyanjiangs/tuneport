//! TunePort 核心:ncm / qmc / kwm 解密 + 标签写入 + 批量引擎。
//!
//! 这一层不依赖 Tauri,可以单独 `cargo test`。

pub mod engine;
pub mod naming;
pub mod ncm;
pub mod tags;

pub use ncm::{AudioFormat, Meta, Ncm, NcmError};

pub mod qmc;

pub mod kwm;
