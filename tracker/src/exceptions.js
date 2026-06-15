// Client-side exception reporting. The agent groups occurrences Sentry-style by a
// server-computed fingerprint, so the client only needs to send the raw type,
// message and stack — short keys mirror the `/track/exception` contract.

const MAX_STACK = 16000;

// Normalise an arbitrary thrown value (an Error, an error-like object, or a bare
// primitive from a promise rejection) into { name, message, stack }.
export function describeError(value, fallbackName) {
  if (value && typeof value === "object" && typeof value.message === "string") {
    return {
      name:
        typeof value.name === "string" && value.name
          ? value.name
          : fallbackName || "Error",
      message: value.message,
      stack: typeof value.stack === "string" ? value.stack : undefined,
    };
  }
  return {
    name: fallbackName || "Error",
    message: stringifyReason(value),
    stack: undefined,
  };
}

function stringifyReason(value) {
  if (value === undefined) return "undefined";
  if (value === null) return "null";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) || String(value);
  } catch (e) {
    return String(value);
  }
}

// Build the `/track/exception` payload. `meta` is expected to already be a
// string→string map (or undefined).
export function buildExceptionPayload(desc, opts) {
  opts = opts || {};
  const payload = {
    u: opts.url,
    ty: desc.name || "Error",
    m: desc.message || desc.name || "Error",
    h: !!opts.handled,
  };
  if (opts.beacon) payload.b = opts.beacon;
  if (desc.stack) {
    payload.s =
      desc.stack.length > MAX_STACK ? desc.stack.slice(0, MAX_STACK) : desc.stack;
  }
  if (opts.meta) payload.d = opts.meta;
  return payload;
}

// A reporter with a dedup set and a hard per-view cap, so a tight error loop can't
// flood the endpoint. `url` and `beacon` are getters because they change across SPA
// navigations. `send` receives the finished payload.
export function createExceptionReporter(opts) {
  const send = opts.send;
  const getUrl = opts.url;
  const getBeacon = opts.beacon;
  const max = opts.max || 25;
  const seen = new Set();
  let count = 0;

  function report(value, handled, meta, fallbackName) {
    if (count >= max) return;
    const desc = describeError(value, fallbackName);
    // Dedup on type + message + the first stack frame.
    const firstFrame = desc.stack ? desc.stack.split("\n", 1)[0] : "";
    const signature = desc.name + "\n" + desc.message + "\n" + firstFrame;
    if (seen.has(signature)) return;
    seen.add(signature);
    count++;
    send(
      buildExceptionPayload(desc, {
        url: getUrl(),
        beacon: getBeacon(),
        handled: handled,
        meta: meta,
      }),
    );
  }

  return { report: report };
}
