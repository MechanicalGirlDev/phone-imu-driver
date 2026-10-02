/**
 * Sensor capture → normalization → WS sending, plus the status display (plain DOM).
 *
 * - devicemotion (≈60Hz) drives the sending pace, and the latest deviceorientation attitude is attached.
 * - On Android, deviceorientationabsolute (magnetic-north reference) is preferred. iOS only has the
 *   non-absolute alpha (aligning the yaw zero point is the consuming driver's job on the server side).
 * - iOS requires a user gesture for requestPermission(), so start() must be called from the
 *   button's click handler.
 * - Mock mode: for PC browsers without sensors, streams a synthetic wave at 60Hz (for UI development
 *   and connectivity checks).
 */

import { encodeSampleFrame, helloMessage, parseServerMessage } from "./protocol.js";
import { detectIOS, eulerToQuat, normalizeAccel, normalizeRotationRate } from "./sensors.js";

const $ = (id) => document.getElementById(id);
const el = {
  url: $("ws-url"),
  mock: $("mock"),
  toggle: $("toggle"),
  error: $("error"),
  serverStatus: $("server-status"),
  conn: $("conn"),
  rate: $("rate"),
  platform: $("platform"),
  seen: $("seen"),
  values: $("values"),
};

// The page is served from the same origin as the WebSocket, so the default is location.host.
el.url.value = `wss://${window.location.host}/ws`;

/** @type {(() => void)[] | null} Non-null only while running. */
let cleanups = null;

function setMessage(node, text) {
  node.textContent = text ?? "";
  node.hidden = !text;
}

function fmt3(v, digits) {
  if (!v) return "—";
  return v.map((x) => x.toFixed(digits).padStart(digits + 4)).join(" ");
}

function renderValues(sample) {
  el.values.textContent =
    `gyro  [rad/s] ${fmt3(sample?.gyro, 3)}\n` +
    `accel [m/s²] ${fmt3(sample?.accel, 2)}\n` +
    `quat  (wxyz) ${fmt3(sample?.quat, 3)}`;
}
renderValues(null);

function setRunning(running) {
  el.toggle.textContent = running ? "Stop" : "Allow sensors and start";
  el.toggle.classList.toggle("stop", running);
  el.url.disabled = running;
  el.mock.disabled = running;
}

function setConnected(open) {
  el.conn.textContent = open ? "Connected" : "Disconnected";
  el.conn.className = open ? "ok" : "err";
}

function stop() {
  if (cleanups) {
    for (const c of cleanups.reverse()) c();
    cleanups = null;
  }
  setRunning(false);
  setConnected(false);
  el.rate.textContent = "0 Hz";
}

async function start(wsUrl, mock) {
  stop();
  setMessage(el.error, null);
  const isIOS = detectIOS(navigator.userAgent, navigator.maxTouchPoints ?? 0);
  const platform = isIOS ? "ios" : /Android/i.test(navigator.userAgent) ? "android" : "unknown";
  el.platform.textContent = platform;
  setRunning(true);
  // cleanups is itself the "running" flag. Materialize it before the await so that pressing the
  // button again while the iOS permission dialog is pending does not re-enter start.
  const pending = [];
  cleanups = pending;

  // --- iOS: obtain permission during the user gesture (this function is expected to be called from a button click).
  if (!mock) {
    try {
      const motionReq = DeviceMotionEvent.requestPermission;
      if (typeof motionReq === "function") {
        const r = await motionReq.call(DeviceMotionEvent);
        if (r !== "granted") throw new Error(`Motion sensor access was not granted (${r})`);
      }
      const orientReq = DeviceOrientationEvent.requestPermission;
      if (typeof orientReq === "function") {
        await orientReq.call(DeviceOrientationEvent).catch(() => undefined);
      }
    } catch (e) {
      setMessage(el.error, String(e));
      stop();
      return;
    }
  }

  // --- WS connection.
  const ws = new WebSocket(wsUrl);
  ws.binaryType = "arraybuffer";
  pending.push(() => ws.close());
  ws.onopen = () => {
    ws.send(helloMessage(platform, navigator.userAgent));
    setConnected(true);
  };
  ws.onclose = () => {
    setConnected(false);
    // Stop only while our own session is still current (so a late close from an old, already
    // closed socket cannot kill a new session after start() reconnected).
    if (cleanups === pending) stop();
  };
  ws.onerror = () => {
    setConnected(false);
    setMessage(
      el.error,
      `WebSocket connection failed (${wsUrl}). Did you accept the certificate?`,
    );
  };
  ws.onmessage = (ev) => {
    if (typeof ev.data !== "string") return;
    const msg = parseServerMessage(ev.data);
    if (msg?.type === "status") {
      setMessage(el.serverStatus, `Server: ${msg.message}`);
    }
  };

  // --- Latest attitude (orientation events arrive independently of motion events).
  let quat = [1, 0, 0, 0];
  let orientationSeen = false;
  const onOrientation = (ev) => {
    if (ev.alpha === null && ev.beta === null && ev.gamma === null) return;
    quat = eulerToQuat(ev.alpha ?? 0, ev.beta ?? 0, ev.gamma ?? 0);
    orientationSeen = true;
  };
  // Android: prefer the magnetic-north absolute event. Otherwise the plain event (iOS only has this one).
  const orientationEvent =
    "ondeviceorientationabsolute" in window ? "deviceorientationabsolute" : "deviceorientation";

  let sent = 0;
  let lastSample = null;
  const sendSample = (sample) => {
    lastSample = sample;
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(encodeSampleFrame(sample, performance.timeOrigin + performance.now()));
      sent += 1;
    }
  };

  let motionSeen = false;
  if (mock) {
    // Synthetic wave: rotates slowly, with gravity plus noise. 60Hz.
    const t0 = performance.now();
    const timer = setInterval(() => {
      const t = (performance.now() - t0) / 1000;
      const yawDeg = (t * 20) % 360;
      motionSeen = true;
      orientationSeen = true;
      sendSample({
        gyro: [0.05 * Math.sin(t), 0.05 * Math.cos(t), (20 * Math.PI) / 180],
        accel: [0.1 * Math.sin(t * 3), 0.1 * Math.cos(t * 2), 9.81],
        quat: eulerToQuat(yawDeg, 0, 0),
      });
    }, 1000 / 60);
    pending.push(() => clearInterval(timer));
  } else {
    const onMotion = (ev) => {
      motionSeen = true;
      const rate = ev.rotationRate ?? { alpha: null, beta: null, gamma: null };
      const acc = ev.accelerationIncludingGravity ?? { x: null, y: null, z: null };
      sendSample({
        gyro: normalizeRotationRate(rate),
        accel: normalizeAccel(acc, isIOS),
        quat,
      });
    };
    window.addEventListener(orientationEvent, onOrientation);
    window.addEventListener("devicemotion", onMotion);
    pending.push(() => {
      window.removeEventListener(orientationEvent, onOrientation);
      window.removeEventListener("devicemotion", onMotion);
    });
  }

  // --- Keep the screen awake (supported browsers only, failures are ignored).
  let wakeLock = null;
  const acquireWakeLock = async () => {
    try {
      wakeLock = (await navigator.wakeLock?.request("screen")) ?? null;
    } catch {
      wakeLock = null;
    }
  };
  void acquireWakeLock();
  const onVisibility = () => {
    if (document.visibilityState === "visible") void acquireWakeLock();
  };
  document.addEventListener("visibilitychange", onVisibility);
  pending.push(() => {
    document.removeEventListener("visibilitychange", onVisibility);
    void wakeLock?.release().catch(() => undefined);
  });

  // --- Reflect the rate and live values into the UI at 1Hz.
  const uiTimer = setInterval(() => {
    el.rate.textContent = `${sent} Hz`;
    sent = 0;
    el.seen.textContent = `${motionSeen ? "✓" : "×"} / ${orientationSeen ? "✓" : "×"}`;
    renderValues(lastSample);
  }, 1000);
  pending.push(() => clearInterval(uiTimer));
}

el.toggle.addEventListener("click", () => {
  if (cleanups) stop();
  else void start(el.url.value, el.mock.checked);
});
window.addEventListener("pagehide", stop);
