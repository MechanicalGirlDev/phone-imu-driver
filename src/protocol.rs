//! Wire protocol: binary sample frames + JSON control messages.
//!
//! The sensor stream itself (60 Hz) is binary; control traffic (hello / status) is JSON text.
//! All values are assumed to be **already normalized by the client to the spec** (rad/s,
//! m/s², W3C device frame, quaternion device -> ENU); nothing is converted here.

use serde::{Deserialize, Serialize};

/// Leading tag of a binary sample frame.
pub const SAMPLE_FRAME_TAG: u8 = 0x01;

/// Total length of a binary sample frame [bytes]: tag(1) + t_client_ms f64(8) + f32 x 10(40).
pub const SAMPLE_FRAME_LEN: usize = 1 + 8 + 4 * 10;

/// One phone sample (raw **device frame** values, normalized units).
///
/// - frame: W3C device frame (X = screen right, Y = screen up, Z = out of the screen)
/// - `quat_wxyz`: attitude of the device -> ENU (east-north-up) world
/// - `accel_m_s2`: includes the gravity reaction (like `accelerationIncludingGravity`, spec sign)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhoneImuSample {
    /// Client clock timestamp [ms] (equivalent to `performance.timeOrigin + now()`).
    pub t_client_ms: f64,
    /// Angular velocity [rad/s], device frame.
    pub gyro_rad_s: [f64; 3],
    /// Acceleration [m/s²], device frame, including the gravity reaction.
    pub accel_m_s2: [f64; 3],
    /// Fused attitude quaternion `(w, x, y, z)`, device -> ENU.
    pub quat_wxyz: [f64; 4],
}

/// Failure to decode a frame.
// The widest variant is a `usize`; boxing it would cost an allocation to save nothing.
#[allow(variant_size_differences)]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    /// The frame was shorter than one fixed-length sample.
    #[error("sample frame too short: {got} bytes (expected {SAMPLE_FRAME_LEN})")]
    TooShort {
        /// How many bytes the frame actually had.
        got: usize,
    },
    /// The leading tag byte did not name a known frame kind.
    #[error("unknown frame tag 0x{tag:02x}")]
    UnknownTag {
        /// The unrecognized leading tag byte.
        tag: u8,
    },
}

/// Decodes a binary sample frame. The frame length is fixed, and surplus bytes are ignored
/// for future extension (forward compatibility: an old server can read frames that carry
/// new fields).
pub fn parse_sample_frame(buf: &[u8]) -> Result<PhoneImuSample, ProtocolError> {
    if buf.len() < SAMPLE_FRAME_LEN {
        return Err(ProtocolError::TooShort { got: buf.len() });
    }
    if buf[0] != SAMPLE_FRAME_TAG {
        return Err(ProtocolError::UnknownTag { tag: buf[0] });
    }
    let mut off = 1;
    let mut f64_at = |b: &[u8]| -> f64 {
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[off..off + 8]);
        off += 8;
        f64::from_le_bytes(a)
    };
    let t_client_ms = f64_at(buf);
    let mut f32_at = || -> f64 {
        let mut a = [0u8; 4];
        a.copy_from_slice(&buf[off..off + 4]);
        off += 4;
        f32::from_le_bytes(a) as f64
    };
    let gyro_rad_s = [f32_at(), f32_at(), f32_at()];
    let accel_m_s2 = [f32_at(), f32_at(), f32_at()];
    let quat_wxyz = [f32_at(), f32_at(), f32_at(), f32_at()];
    Ok(PhoneImuSample {
        t_client_ms,
        gyro_rad_s,
        accel_m_s2,
        quat_wxyz,
    })
}

/// Encodes a sample into a binary frame (for tests and Rust mock clients; production
/// encoding is done by the browser-side JS `web/protocol.js` - change both together).
pub fn encode_sample_frame(s: &PhoneImuSample) -> [u8; SAMPLE_FRAME_LEN] {
    let mut buf = [0u8; SAMPLE_FRAME_LEN];
    buf[0] = SAMPLE_FRAME_TAG;
    buf[1..9].copy_from_slice(&s.t_client_ms.to_le_bytes());
    let mut off = 9;
    let mut put = |v: f64, buf: &mut [u8; SAMPLE_FRAME_LEN]| {
        buf[off..off + 4].copy_from_slice(&(v as f32).to_le_bytes());
        off += 4;
    };
    for v in s.gyro_rad_s {
        put(v, &mut buf);
    }
    for v in s.accel_m_s2 {
        put(v, &mut buf);
    }
    for v in s.quat_wxyz {
        put(v, &mut buf);
    }
    buf
}

/// JSON control message from the client (phone) to the server.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Self-introduction right after connecting. For platform display and debugging.
    Hello {
        /// `navigator.platform`, when the browser exposes it.
        #[serde(default)]
        platform: Option<String>,
        /// The browser's user-agent string, when it exposes it.
        #[serde(default)]
        ua: Option<String>,
    },
}

/// JSON control message from the server to the client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Connection state notification (for example, superseded by a new connection).
    Status {
        /// Whether this socket is still the active one.
        connected: bool,
        /// Human-readable reason, shown on the phone.
        message: String,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample() -> PhoneImuSample {
        PhoneImuSample {
            t_client_ms: 12345.678,
            gyro_rad_s: [0.1, -0.2, 0.3],
            accel_m_s2: [0.0, 0.5, 9.81],
            quat_wxyz: [1.0, 0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn roundtrip_sample_frame() {
        let s = sample();
        let buf = encode_sample_frame(&s);
        assert_eq!(buf.len(), SAMPLE_FRAME_LEN);
        let got = parse_sample_frame(&buf).unwrap();
        // Values pass through f32, so exact equality only holds up to f32 precision.
        assert_eq!(got.t_client_ms, s.t_client_ms); // t is carried as f64
        for i in 0..3 {
            assert!((got.gyro_rad_s[i] - s.gyro_rad_s[i]).abs() < 1e-6);
            assert!((got.accel_m_s2[i] - s.accel_m_s2[i]).abs() < 1e-6);
        }
        for i in 0..4 {
            assert!((got.quat_wxyz[i] - s.quat_wxyz[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn short_frame_is_rejected() {
        let buf = [SAMPLE_FRAME_TAG; 10];
        assert_eq!(
            parse_sample_frame(&buf),
            Err(ProtocolError::TooShort { got: 10 })
        );
    }

    #[test]
    fn unknown_tag_is_rejected() {
        let mut buf = encode_sample_frame(&sample());
        buf[0] = 0x7f;
        assert_eq!(
            parse_sample_frame(&buf),
            Err(ProtocolError::UnknownTag { tag: 0x7f })
        );
    }

    #[test]
    fn trailing_bytes_are_ignored_for_forward_compat() {
        let base = encode_sample_frame(&sample());
        let mut buf = base.to_vec();
        buf.extend_from_slice(&[0xaa, 0xbb]);
        assert!(parse_sample_frame(&buf).is_ok());
    }

    #[test]
    fn client_messages_parse() {
        let hello: ClientMessage =
            serde_json::from_str(r#"{"type":"hello","platform":"ios","ua":"Safari"}"#).unwrap();
        assert!(matches!(hello, ClientMessage::Hello { platform: Some(p), .. } if p == "ios"));
    }

    #[test]
    fn server_status_serializes() {
        let json = serde_json::to_string(&ServerMessage::Status {
            connected: false,
            message: "m".to_string(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"status","connected":false,"message":"m"}"#);
    }
}
