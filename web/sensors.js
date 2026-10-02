/**
 * Normalization of browser sensor values across devices (pure functions only - the target of node --test).
 *
 * Conventions carried on the wire (paired with src/protocol.rs):
 * - frame: W3C device frame (X = screen right, Y = screen up, Z = out of the screen)
 * - gyro: rad/s, accel: m/s² (includes the gravity reaction, spec sign = z ≈ +9.81 lying flat at rest)
 * - quaternion: device → ENU (east-north-up), (w, x, y, z)
 *
 * Platform differences:
 * - rotationRate is in deg/s on both OSes → converted to rad/s. Axes: alpha=Z, beta=X, gamma=Y.
 * - iOS accelerationIncludingGravity has the opposite sign to the spec (z ≈ -9.81 lying flat at rest) → negated.
 * - Attitude euler angles (alpha, beta, gamma) follow the W3C intrinsic Z-X'-Y'' order → converted to a quaternion.
 *   iOS alpha is non-absolute (relative to the start), but aligning the yaw zero point is the server-side consumer's job.
 *
 * @typedef {[number, number, number]} Vec3
 * @typedef {[number, number, number, number]} QuatWxyz (w, x, y, z)
 * @typedef {{ gyro: Vec3, accel: Vec3, quat: QuatWxyz }} NormalizedSample
 */

const DEG = Math.PI / 180;

/**
 * devicemotion rotationRate (deg/s, alpha=Z/beta=X/gamma=Y) → gyro [rad/s] (x, y, z).
 * @param {{alpha: number|null, beta: number|null, gamma: number|null}} rate
 * @returns {Vec3}
 */
export function normalizeRotationRate(rate) {
  return [(rate.beta ?? 0) * DEG, (rate.gamma ?? 0) * DEG, (rate.alpha ?? 0) * DEG];
}

/**
 * devicemotion accelerationIncludingGravity → accel [m/s²] (spec sign).
 * iOS (WebKit) reports the opposite sign to the spec, so it is negated.
 * @param {{x: number|null, y: number|null, z: number|null}} acc
 * @param {boolean} isIOS
 * @returns {Vec3}
 */
export function normalizeAccel(acc, isIOS) {
  const s = isIOS ? -1 : 1;
  return [s * (acc.x ?? 0), s * (acc.y ?? 0), s * (acc.z ?? 0)];
}

/**
 * deviceorientation euler angles (alpha, beta, gamma) [deg] → quaternion (device → ENU).
 * The W3C-specified intrinsic Tait-Bryan Z-X'-Y'' (same formula as the spec's getQuaternion example).
 * @param {number} alpha
 * @param {number} beta
 * @param {number} gamma
 * @returns {QuatWxyz}
 */
export function eulerToQuat(alpha, beta, gamma) {
  const x = (beta * DEG) / 2;
  const y = (gamma * DEG) / 2;
  const z = (alpha * DEG) / 2;
  const cX = Math.cos(x),
    cY = Math.cos(y),
    cZ = Math.cos(z);
  const sX = Math.sin(x),
    sY = Math.sin(y),
    sZ = Math.sin(z);
  return [
    cX * cY * cZ - sX * sY * sZ,
    sX * cY * cZ - cX * sY * sZ,
    cX * sY * cZ + sX * cY * sZ,
    cX * cY * sZ + sX * sY * cZ,
  ];
}

/**
 * Rotates v by the quaternion (a device vector into ENU). Used to verify in tests.
 * @param {QuatWxyz} q
 * @param {Vec3} v
 * @returns {Vec3}
 */
export function rotateVec(q, v) {
  const [w, qx, qy, qz] = q;
  // t = 2 q_vec × v
  const tx = 2 * (qy * v[2] - qz * v[1]);
  const ty = 2 * (qz * v[0] - qx * v[2]);
  const tz = 2 * (qx * v[1] - qy * v[0]);
  // v' = v + w t + q_vec × t
  return [
    v[0] + w * tx + qy * tz - qz * ty,
    v[1] + w * ty + qz * tx - qx * tz,
    v[2] + w * tz + qx * ty - qy * tx,
  ];
}

/**
 * iOS (WebKit) detection. iPadOS 13+ reports itself as a Mac, so touch support is checked too.
 * @param {string} ua
 * @param {number} maxTouchPoints
 * @returns {boolean}
 */
export function detectIOS(ua, maxTouchPoints) {
  return /iP(hone|ad|od)/.test(ua) || (/Mac/.test(ua) && maxTouchPoints > 1);
}
