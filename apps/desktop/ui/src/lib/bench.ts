// The window, measured: how long the first rows take, how long a jump anywhere in the list takes to show real rows,
// how steady the frames are while the list is flung, and how fast search answers while typing. Run with `?bench`;
// the numbers are posted to the core, which writes them to a file.

export type Drawn = { first: number; last: number; loaded: number };

type Hooks = {
  count: number;
  rowHeight: number;
  /** The rows on screen at the last draw, and how many of them had their data. */
  drawn: () => Drawn;
  scrollToIndex: (index: number) => void;
  scrollBy: (pixels: number) => void;
  type: (text: string) => void;
  /** The query whose results are on screen, or null while the whole library shows. */
  showing: () => string | null;
  /** When the script started, the counts arrived and the first rows had their data (ms since the page began to load). */
  marks: Record<string, number>;
};

const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));

async function until(condition: () => boolean, timeoutMs = 5000): Promise<boolean> {
  const deadline = performance.now() + timeoutMs;
  while (!condition()) {
    if (performance.now() > deadline) return false;
    await frame();
  }
  return true;
}

function spread(values: number[]) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.round((sorted.length - 1) * q)] ?? 0;
  const round = (n: number) => Math.round(n * 10) / 10;
  return { p50: round(at(0.5)), p95: round(at(0.95)), max: round(at(1)), n: sorted.length };
}

/** Scrolls `pixels` every frame for `frames` frames and reports the frame times and how much of the screen was blank. */
async function scrollRun(hooks: Hooks, pixels: number, frames: number) {
  const times: number[] = [];
  const blank: number[] = [];
  let previous = await frame();
  for (let i = 0; i < frames; i++) {
    hooks.scrollBy(pixels);
    const now = await frame();
    times.push(now - previous);
    previous = now;
    const d = hooks.drawn();
    const onScreen = Math.max(1, d.last - d.first);
    blank.push((onScreen - d.loaded) / onScreen);
  }
  const over = (ms: number) => Math.round((times.filter((t) => t > ms).length / times.length) * 1000) / 10;
  return {
    rowsPerSecond: Math.round((pixels / hooks.rowHeight) * (1000 / (times.reduce((a, b) => a + b, 0) / times.length))),
    frameMs: spread(times),
    framesOver20msPercent: over(20),
    framesOver34msPercent: over(34),
    blankShareOfScreen: { average: Math.round((blank.reduce((a, b) => a + b, 0) / blank.length) * 1000) / 1000, worst: Math.max(...blank) },
  };
}

export async function measure(hooks: Hooks) {
  const full = () => {
    const d = hooks.drawn();
    return d.last > d.first && d.loaded === d.last - d.first;
  };

  // First rows: from the page starting to load until the first screenful has its data and a frame has been drawn.
  await until(full);
  await frame();
  // The marks are taken when things happen; `painted` also waits for a frame, which a hidden window does not get.
  const firstRows = { ...hooks.marks, paintedMs: Math.round(performance.now()), visibility: document.visibilityState };

  // Jumps: the scrollbar dragged to thirty places all over the library.
  const jumps: number[] = [];
  let seed = 12345;
  for (let i = 0; i < 30; i++) {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    const index = Math.floor((seed / 2147483648) * (hooks.count - 200));
    const started = performance.now();
    hooks.scrollToIndex(index);
    await until(() => Math.abs(hooks.drawn().first - index) <= 1 && full());
    await frame();
    jumps.push(performance.now() - started);
  }

  // Scrolling: a steady scroll and a hard fling, from the top.
  hooks.scrollToIndex(0);
  await until(full);
  const steady = await scrollRun(hooks, 3 * hooks.rowHeight, 240);
  hooks.scrollToIndex(0);
  await until(full);
  const fling = await scrollRun(hooks, 30 * hooks.rowHeight, 240);

  // Search: a query typed letter by letter; each letter is timed until its own results are on screen.
  const typing: number[] = [];
  const text = "golden river";
  for (let i = 1; i <= text.length; i++) {
    const typed = text.slice(0, i);
    if (typed.trim() !== typed) continue;
    const started = performance.now();
    hooks.type(typed);
    await until(() => hooks.showing() === typed);
    await frame();
    typing.push(performance.now() - started);
  }
  hooks.type("");
  await until(() => hooks.showing() === null);

  const memory = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory;
  return {
    userAgent: navigator.userAgent,
    window: { width: innerWidth, height: innerHeight, devicePixelRatio },
    library: { tracks: hooks.count },
    rowsOnScreen: hooks.drawn().last - hooks.drawn().first,
    firstRows,
    jumpToRowsMs: spread(jumps),
    steadyScroll: steady,
    fling,
    searchWhileTypingMs: spread(typing),
    jsHeapMB: memory ? Math.round((memory.usedJSHeapSize / 1048576) * 10) / 10 : null,
  };
}
