# phone-imu-driver

Use a smartphone browser as an IMU source. The `phone-imu-ws` crate runs a single-origin
HTTPS server that serves a static web UI and a `/ws` WebSocket. The web UI streams
DeviceMotion / DeviceOrientation samples (normalized to rad/s, m/s² and a device -> ENU
quaternion) as compact binary frames, and the Rust side keeps the latest sample for a robot
driver to read.

- Crate / package: `phone-imu-ws` (library `phone_imu_ws`, binary `phone-imu`)
- Version: 0.1.4, edition 2024, Rust toolchain 1.97
- License: Apache-2.0 (see `LICENSE` and `NOTICE`)
- Author: nop <noplab90@gmail.com>, MechanicalGirl LLC

## Run the standalone tool

```text
cargo run --bin phone-imu -- --web-root web
```

Options: `--listen` (default `0.0.0.0:8443`), `--web-root` (static web directory, default:
a built-in placeholder page), `--cert-dir` (default `.cert`).

## Web assets

The web UI lives in [`web/`](web/) (plain HTML + ES modules, no build step, no frontend
dependencies). It is not embedded in the binary: pass the directory with
`--web-root web` from the repository root, or an absolute path to a copy of `web/` when
the binary runs elsewhere. Without `--web-root` (or when the path does not exist) only the
placeholder page is served, but `/ws` still works.

## Live HTTPS/WSS check

1. Run `cargo run --bin phone-imu -- --web-root web` on the PC.
2. Open `https://<PC LAN IP>:8443/` on the phone (same WiFi) and accept the self-signed
   certificate warning once. The certificate is generated on first start in `.cert/`
   (delete the directory to regenerate it after the PC's IP changes).
3. Tap "Allow sensors and start". The CLI prints the rate and values once per second.
   On a PC browser without sensors, tick "Mock sensors" to stream a synthetic wave.

The page connects to `wss://<same host>/ws`; keeping the page and the socket on one origin
is what lets the certificate exception cover both.

## Library use

```rust,ignore
let (handle, server) = phone_imu_ws::spawn(phone_imu_ws::PhoneImuServerConfig {
    listen: "0.0.0.0:8443".parse()?,
    web_root: Some("web".into()),
    cert_dir: ".cert".into(),
})
.await?;
let latest = handle.latest(); // raw device-frame sample with receive time and connection id
```

## Reiny 0.7 integration

`phone-imu-ws` is the HTTPS/WebSocket transport, and `phone-imu` remains a
standalone connectivity tool. Reiny message schemas and deployment ownership
belong to a consuming adapter.

That adapter declares its typed IMU output in `main.yaml`, opens the named
`Cloudy::output`, awaits `spawn` so certificate preparation and listener binding
have succeeded, and then calls `Cloudy::ready()`. Phone connection availability,
device-to-body conversion, sample age and connection-generation changes remain
explicit application policies. Preserve the deployment/module namespace as
publisher provenance.

Retain the returned server task. On `Cloudy::shutdown()`, abort and await it so
the adapter releases its listener before exiting; dropping the task handle
alone does not cancel the server.

## Development

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
node --test web/sensors.test.mjs
```

The wire format is defined in `src/protocol.rs` and mirrored in `web/protocol.js`; change
both together.
