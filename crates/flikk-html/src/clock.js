// Injected before every page script (Page.addScriptToEvaluateOnNewDocument).
// The engine owns time: performance.now, Date, timers and
// requestAnimationFrame follow a virtual clock that only moves when the
// engine calls `__flikk.frame(state)`, and every animation is paused and
// seeked to that clock before a capture.
(() => {
  if (window.__flikk) return;
  const EPOCH = Date.UTC(2025, 0, 1);
  const FRAME = 1000 / 60;
  const MAX_CALLS = 100000;
  const SEED = 0x2545f491;

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

  window.flikk = { time: 0, duration: 0, progress: 0, width: innerWidth, height: innerHeight, params: {} };

  const fail = (e) => errors.push(e && e.stack ? String(e.stack) : String(e));
  const call = (fn, args) => {
    try {
      if (typeof fn === 'function') return fn(...args);
      (0, eval)(String(fn));
    } catch (e) {
      fail(e);
    }
  };

  // Deterministic randomness (mulberry32), reset with every load and build.
  let seed = SEED;
  Math.random = () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };

  Object.defineProperty(performance, 'now', { value: () => now, configurable: true });
  const RealDate = Date;
  function FlikkDate(...a) {
    if (!new.target) return new RealDate(EPOCH + now).toString();
    return Reflect.construct(RealDate, a.length ? a : [EPOCH + now], new.target);
  }
  Object.setPrototypeOf(FlikkDate, RealDate);
  FlikkDate.prototype = RealDate.prototype;
  FlikkDate.now = () => EPOCH + now;
  window.Date = FlikkDate;

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
    const id = nextId++;
    frames.set(id, cb);
    return id;
  };
  window.cancelAnimationFrame = (id) => void frames.delete(id);

  // GSAP (preloaded, or loaded by the page) renders on this clock: each
  // copy's root timeline is rendered at the clip time instead of from its
  // ticker, and keeps finished animations, so seeking back needs no reload.
  const gsaps = [];
  const listened = (ticker) => (ticker._listeners || []).length > 0;
  const adopt = (g) => {
    if (!g || !g.ticker || !g.updateRoot || !g.globalTimeline || gsaps.includes(g)) return;
    gsaps.push(g);
    const ticker = g.ticker;
    ticker.remove(g.updateRoot);
    g.globalTimeline.autoRemoveChildren = false;
    // It wakes for every new animation; only the page's own listeners keep it up.
    const wake = ticker.wake;
    ticker.wake = () => {
      wake.call(ticker);
      if (!listened(ticker)) ticker.sleep();
    };
    // Adopted while waking for the first time.
    queueMicrotask(() => listened(ticker) || ticker.sleep());
  };
  let gsap;
  Object.defineProperty(window, 'gsap', {
    configurable: true,
    enumerable: true,
    get: () => gsap,
    set: (g) => {
      gsap = g;
      adopt(g);
    },
  });
  // Time 0 renders as just after it, so what is set at 0 shows on the first frame.
  const renderGsap = () => {
    const t = Math.max(now / 1000, 1e-6);
    for (const g of gsaps) {
      try {
        g.updateRoot(t);
      } catch (e) {
        fail(e);
      }
    }
  };
  // Seeking back replays from 0, as a fresh page plays forward. Not
  // `globalTimeline.totalTime(0)`: that realigns the root to the ticker.
  const rewindGsap = () => {
    for (const g of gsaps) {
      try {
        g.updateRoot(0);
      } catch (e) {
        fail(e);
      }
    }
  };

  // `flikkTimeline(flikk)` builds the page's GSAP animations, in a context so
  // that a rebuild for other params, duration or size reverts the last one.
  // Seeking back rebuilds too: tweens that were never rendered and tweens
  // rewound to their start differ at the instant they start.
  let built = '';
  let context = null;
  const stale = (state, back) => {
    if (typeof window.flikkTimeline !== 'function' || !window.gsap || !window.gsap.context) return '';
    const { time, ...rest } = state;
    const key = JSON.stringify(rest);
    return key !== built || back ? key : '';
  };
  const build = (key) => {
    built = key;
    if (context) context.revert();
    // With the root at 0, what the build creates starts with the clip.
    rewindGsap();
    seed = SEED;
    context = window.gsap.context(() => call(window.flikkTimeline, [window.flikk]));
  };

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
    const k = window.flikk;
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
      renderGsap();
      call(next.fn, next.args);
      note();
    }
  };
  const runFrames = () => {
    if (!frames.size || ++calls > MAX_CALLS) return;
    usedClock = true;
    const due = frames;
    frames = new Map();
    sync();
    renderGsap();
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

  window.__flikk = {
    errors,
    // `state`: { time, duration, width, height, params, vars }.
    apply(state) {
      window.flikk = {
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
      const back = target < now;
      const key = stale(state, back);
      if (key) {
        // Builds measure text and pictures.
        await ready();
        build(key);
      }
      if (back) {
        now = target;
        rewindGsap();
      } else advance(target);
      sync();
      renderGsap();
      seekAnimations();
      if (typeof window.flikkRender === 'function') call(window.flikkRender, [now / 1000]);
      seekAnimations();
      await ready();
      return 'ok';
    },
  };
})();
