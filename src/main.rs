//! Standalone connectivity CLI: starts the server and dumps received samples and the sample
//! rate once per second.
//!
//! It verifies the phone-to-PC sensor pipeline without any robot attached.
//!
//! ```text
//! cargo run --bin phone-imu -- --web-root web
//! ```

use core::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use phone_imu_ws::{PhoneImuServerConfig, spawn};

#[derive(Debug, Parser)]
#[command(
    name = "phone-imu",
    about = "Smartphone IMU WebSocket server (standalone dump tool)"
)]
struct Args {
    /// HTTPS/WSS listen address.
    #[arg(long, default_value = "0.0.0.0:8443")]
    listen: SocketAddr,
    /// Static web root (this repository's `web/` dir, served as-is). Omit to serve a placeholder page.
    #[arg(long)]
    web_root: Option<PathBuf>,
    /// Directory for the generated self-signed TLS certificate.
    #[arg(long, default_value = ".cert")]
    cert_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let (handle, server) = spawn(PhoneImuServerConfig {
        listen: args.listen,
        web_root: args.web_root,
        cert_dir: args.cert_dir,
    })
    .await?;

    let dump = tokio::spawn(async move {
        let mut last_count = 0u64;
        let mut tick = tokio::time::interval(core::time::Duration::from_secs(1));
        loop {
            let _ = tick.tick().await;
            let count = handle.sample_count();
            let rate = count.saturating_sub(last_count);
            last_count = count;
            match handle.latest() {
                Some(s) if handle.connected() => {
                    let q = s.sample.quat_wxyz;
                    let g = s.sample.gyro_rad_s;
                    let a = s.sample.accel_m_s2;
                    println!(
                        "conn#{} {:>3} Hz gyro=[{:+.3} {:+.3} {:+.3}] acc=[{:+.2} {:+.2} {:+.2}] quat=[{:+.3} {:+.3} {:+.3} {:+.3}] age={:.0}ms",
                        s.connection_id,
                        rate,
                        g[0],
                        g[1],
                        g[2],
                        a[0],
                        a[1],
                        a[2],
                        q[0],
                        q[1],
                        q[2],
                        q[3],
                        s.received.elapsed().as_secs_f64() * 1000.0,
                    );
                }
                _ => println!("(no phone connected)"),
            }
        }
    });

    tokio::select! {
        r = server => r?,
        r = dump => r?,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}
