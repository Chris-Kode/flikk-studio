// Injected before every page script (Page.addScriptToEvaluateOnNewDocument).
// The engine owns time: performance.now, Date, timers and
// requestAnimationFrame follow a virtual clock that only moves when the
// engine calls `__koo.frame(state)`, and every animation is paused and
// seeked to that clock before a capture.
(() => {
  if (window.__koo) return;
  const EPOCH = Date.UTC(2025, 0, 1);
  const FRAME = 1000 / 60;
  const MAX_CALLS = 100000;

  let now = 0; // virtual ms since the clip start
  let usedClock = false; // time-driven JS ran: seeking back needs a reload
  let nextId = 1;
  let timers = new Map(); // id -> { due, fn, args, every }
  let frames = new Map(); // id -> callback
  const births = new WeakMap(); // Animation -> virtual ms when first seen
  const decoded = new WeakSet();
  const errors = [];
  const sheet = new CSSStyleSheet();
  try {
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  } catch (e) {}

  window.koo = { time: 0, duration: 0, progress: 0, width: innerWidth, height: innerHeight, params: {} };

  const fail = (e) => errors.push(e && e.stack ? String(e.stack) : String(e));
  const call = (fn, args) => {
    try {
      if (typeof fn === 'function') fn(...args);
      else (0, eval)(String(fn));
    } catch (e) {
      fail(e);
    }
  };

  // Deterministic randomness (mulberry32), reset with every load.
  let seed = 0x2545f491;
  Math.random = () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };

  Object.defineProperty(performance, 'now', { value: () => now, configurable: true });
  const RealDate = Date;
  function KooDate(...a) {
    if (!new.target) return new RealDate(EPOCH + now).toString();
    return Reflect.construct(RealDate, a.length ? a : [EPOCH + now], new.target);
  }
  Object.setPrototypeOf(KooDate, RealDate);
  KooDate.prototype = RealDate.prototype;
  KooDate.now = () => EPOCH + now;
  window.Date = KooDate;

  window.setTimeout = (fn, ms, ...args) => {
    const due = now + Math.max(0, +ms || 0);
    if (due > 0) usedClock = true;
    const id = nextId++;
    timers.set(id, { due, fn, args, every: 0 });
    return id;
  };
  window.setInterval = (fn, ms, ...args) => {
    usedClock = true;
    const every = Math.max(1, +ms || 0);
    const id = nextId++;
    timers.set(id, { due: now + every, fn, args, every });
    return id;
  };
  window.clearTimeout = window.clearInterval = (id) => void timers.delete(id);
  window.requestAnimationFrame = (cb) => {
    usedClock = true;
    const id = nextId++;
    frames.set(id, cb);
    return id;
  };
  window.cancelAnimationFrame = (id) => void frames.delete(id);

  const animations = () => (document.getAnimations ? document.getAnimations() : []);
  const note = () => {
    for (const a of animations()) if (!births.has(a)) births.set(a, now);
  };
  const seekAnimations = () => {
    for (const a of animations()) {
      if (!births.has(a)) births.set(a, now);
      try {
        a.pause();
        a.currentTime = Math.max(0, now - births.get(a));
      } catch (e) {}
    }
  };
  // Nothing runs on the wall clock between loading and the first frame.
  document.addEventListener('DOMContentLoaded', seekAnimations);
  window.addEventListener('load', seekAnimations);

  const sync = () => {
    const k = window.koo;
    k.time = now / 1000;
    k.progress = k.duration > 0 ? Math.min(1, Math.max(0, k.time / k.duration)) : 0;
  };

  let calls = 0;
  const runTimers = (upTo) => {
    for (;;) {
      let id = 0;
      let next = null;
      for (const [i, t] of timers) {
        if (t.due <= upTo && (!next || t.due < next.due)) {
          id = i;
          next = t;
        }
      }
      if (!next || ++calls > MAX_CALLS) return;
      now = Math.max(now, next.due);
      if (next.every) next.due += next.every;
      else timers.delete(id);
      sync();
      call(next.fn, next.args);
      note();
    }
  };
  const runFrames = () => {
    if (!frames.size || ++calls > MAX_CALLS) return;
    const due = frames;
    frames = new Map();
    sync();
    for (const cb of due.values()) call(cb, [now]);
    note();
  };
  const nextTimer = () => {
    let due = Infinity;
    for (const t of timers.values()) due = Math.min(due, t.due);
    return due;
  };
  const advance = (target) => {
    calls = 0;
    note();
    while (now < target && calls <= MAX_CALLS) {
      const step = frames.size
        ? Math.min(target, (Math.floor(now / FRAME + 1e-9) + 1) * FRAME)
        : Math.min(target, nextTimer());
      runTimers(step);
      now = step;
      if (step < target) runFrames();
    }
    now = target;
    runTimers(target);
    runFrames();
  };

  const ready = async () => {
    try {
      await document.fonts.ready;
    } catch (e) {}
    const pending = [];
    for (const img of document.images) {
      if (decoded.has(img) && img.complete) continue;
      decoded.add(img);
      pending.push(img.decode().catch(() => {}));
    }
    await Promise.all(pending);
  };

  window.__koo = {
    errors,
    // `state`: { time, duration, width, height, params, vars }.
    apply(state) {
      window.koo = {
        time: now / 1000,
        duration: state.duration,
        progress: 0,
        width: state.width,
        height: state.height,
        params: state.params,
      };
      sync();
      try {
        sheet.replaceSync(':root{' + state.vars + '}');
      } catch (e) {
        fail(e);
      }
    },
    async frame(state) {
      const target = Math.max(0, state.time * 1000);
      if (target < now && usedClock) return 'reload';
      this.apply(state);
      if (target < now) now = target;
      else advance(target);
      sync();
      seekAnimations();
      if (typeof window.kooRender === 'function') call(window.kooRender, [now / 1000]);
      seekAnimations();
      await ready();
      return 'ok';
    },
  };
})();
