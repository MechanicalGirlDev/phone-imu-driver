/**
 * Wire protocol encoding/decoding.
 * Change this together with the Rust source of truth (src/protocol.rs).
 *
 * Binary sample frame (little-endian, 49 bytes):
 *   [u8 tag=0x01][f64 t_client_ms][f32×3 gyro rad/s][f32×3 accel m/s²][f32×4 quat w,x,y,z]
 *
 * @typedef {import("./sensors.js").NormalizedSample} NormalizedSample
 * @typedef {{type: "status", connected: boolean, message: string}} ServerMessage
 */

export const SAMPLE_FRAME_TAG = 0x01;
export const SAMPLE_FRAME_LEN = 1 + 8 + 4 * 10;

/**
 * @param {NormalizedSample} sample
 * @param {number} tClientMs
 * @returns {ArrayBuffer}
 */
export function encodeSampleFrame(sample, tClientMs) {
  const buf = new ArrayBuffer(SAMPLE_FRAME_LEN);
  const view = new DataView(buf);
  view.setUint8(0, SAMPLE_FRAME_TAG);
  view.setFloat64(1, tClientMs, true);
  let off = 9;
  const put = (v) => {
    view.setFloat32(off, v, true);
    off += 4;
  };
  sample.gyro.forEach(put);
  sample.accel.forEach(put);
  sample.quat.forEach(put);
  return buf;
}

/**
 * JSON control message from the server to the client.
 * @param {string} text
 * @returns {ServerMessage | null}
 */
export function parseServerMessage(text) {
  try {
    const msg = JSON.parse(text);
    if (msg?.type === "status") return msg;
    return null;
  } catch {
    return null;
  }
}

/**
 * @param {"ios" | "android" | "unknown"} platform
 * @param {string} ua
 * @returns {string}
 */
export function helloMessage(platform, ua) {
  return JSON.stringify({ type: "hello", platform, ua });
}
