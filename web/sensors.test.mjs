import assert from "node:assert/strict";
import { test } from "node:test";

import {
  encodeSampleFrame,
  parseServerMessage,
  SAMPLE_FRAME_LEN,
  SAMPLE_FRAME_TAG,
} from "./protocol.js";
import {
  detectIOS,
  eulerToQuat,
  normalizeAccel,
  normalizeRotationRate,
  rotateVec,
} from "./sensors.js";

function approx(a, b, eps = 1e-9) {
  assert.equal(a.length, b.length);
  for (const [i, v] of a.entries())
    assert.ok(Math.abs(v - b[i]) < eps, `${v} !≈ ${b[i]} (index ${i})`);
}

test("normalizeRotationRate maps alpha/beta/gamma (deg/s) to x/y/z rad/s", () => {
  // alpha = around the Z axis, beta = X, gamma = Y.
  approx(normalizeRotationRate({ alpha: 90, beta: 180, gamma: -90 }), [
    Math.PI,
    -Math.PI / 2,
    Math.PI / 2,
  ]);
});

test("normalizeRotationRate treats null axes as zero", () => {
  approx(normalizeRotationRate({ alpha: null, beta: null, gamma: null }), [0, 0, 0]);
});

test("normalizeAccel keeps spec sign on android (flat at rest: z ≈ +9.81)", () => {
  approx(normalizeAccel({ x: 0, y: 0, z: 9.81 }, false), [0, 0, 9.81]);
});

test("normalizeAccel flips the inverted iOS sign back to spec", () => {
  // iOS reports z ≈ -9.81 lying flat at rest → back to the spec's +9.81.
  approx(normalizeAccel({ x: 0.1, y: -0.2, z: -9.81 }, true), [-0.1, 0.2, 9.81]);
});

test("eulerToQuat: identity euler is identity quaternion", () => {
  approx(eulerToQuat(0, 0, 0), [1, 0, 0, 0]);
});

test("eulerToQuat: alpha rotates the device X axis toward north (ENU yaw)", () => {
  // alpha=90°: a device lying screen-up turned 90° counter-clockwise → device X (which pointed east) now points north.
  approx(rotateVec(eulerToQuat(90, 0, 0), [1, 0, 0]), [0, 1, 0], 1e-12);
});

test("eulerToQuat: beta=90 tilts the device Y axis up", () => {
  // Stand the device upright (screen facing you): device Y (screen up) points to the zenith.
  approx(rotateVec(eulerToQuat(0, 90, 0), [0, 1, 0]), [0, 0, 1], 1e-12);
});

test("eulerToQuat: flat at rest maps device Z to ENU up", () => {
  approx(rotateVec(eulerToQuat(33, 0, 0), [0, 0, 1]), [0, 0, 1], 1e-12); // yaw alone leaves the Z axis fixed
});

test("encodeSampleFrame writes the 49-byte little-endian layout", () => {
  const buf = encodeSampleFrame(
    { gyro: [0.1, -0.2, 0.3], accel: [0, 0.5, 9.81], quat: [1, 0, 0, 0] },
    12345.678,
  );
  assert.equal(buf.byteLength, SAMPLE_FRAME_LEN);
  const view = new DataView(buf);
  assert.equal(view.getUint8(0), SAMPLE_FRAME_TAG);
  assert.ok(Math.abs(view.getFloat64(1, true) - 12345.678) < 1e-6);
  assert.ok(Math.abs(view.getFloat32(9, true) - 0.1) < 1e-6); // gyro.x
  assert.ok(Math.abs(view.getFloat32(9 + 4 * 5, true) - 9.81) < 1e-5); // accel.z
  assert.ok(Math.abs(view.getFloat32(9 + 4 * 6, true) - 1.0) < 1e-6); // quat.w
});

test("parseServerMessage parses status, rejects junk", () => {
  assert.deepEqual(parseServerMessage('{"type":"status","connected":false,"message":"m"}'), {
    type: "status",
    connected: false,
    message: "m",
  });
  assert.equal(parseServerMessage("not json"), null);
  assert.equal(parseServerMessage('{"type":"other"}'), null);
});

test("detectIOS detects iPhone / iPad / desktop", () => {
  assert.equal(detectIOS("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)", 5), true);
  // iPadOS 13+ claims to be a Macintosh but has touch support.
  assert.equal(detectIOS("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 5), true);
  assert.equal(detectIOS("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 0), false);
  assert.equal(detectIOS("Mozilla/5.0 (Linux; Android 14; Pixel 8)", 5), false);
});
