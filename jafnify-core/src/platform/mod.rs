//! Platform sensor seams. Pure mapping functions are always compiled and
//! unit-tested; the OS readers behind them are `cfg(windows)`.

pub mod station;
pub mod uplink;
