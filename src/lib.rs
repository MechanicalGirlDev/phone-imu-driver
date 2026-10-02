//! Transport crate that turns a smartphone browser's sensors into an IMU source.
//!
//! On the phone, the `web/` directory (plain HTML + ESM, no build step) reads
//! DeviceMotion / DeviceOrientation, normalizes platform differences, and sends WebSocket
//! binary frames. This crate is the receiving end:
//!
//! - A **single-origin HTTPS server** ([`server::spawn`]) serves the static page and `/ws`
//!   on the same listen port. DeviceMotion requires a secure context (HTTPS), and a
//!   WebSocket opened from an HTTPS page must use wss:// - a self-signed certificate
//!   exception also applies to wss when the origin is the same, so the page and the
//!   WebSocket must not be split across ports.
//! - On first start, [`cert`] generates a self-signed certificate and persists it in
//!   `cert_dir`.
//! - Received samples are kept as raw device-frame values (latest wins) and can be read
//!   synchronously through [`PhoneImuHandle`] (`latest()`). Conversion to the robot body
//!   frame and freshness checks are the responsibility of the consuming driver.
//!
//! The crate has no dependency on any robot framework; it is a pure transport layer.

extern crate alloc;

pub mod cert;
pub mod protocol;
pub mod server;

pub use protocol::{ClientMessage, PhoneImuSample, ProtocolError, ServerMessage};
pub use server::{PhoneImuHandle, PhoneImuServerConfig, ServeError, StampedSample, spawn};
