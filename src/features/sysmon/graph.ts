// The history graphs: plain Canvas 2D in the token colours. Drawn only when a
// reading arrives while the section is on screen; nothing here has a timer.

/** Points a full graph holds (svc-sysmon keeps as many). */
export const HISTORY = 120;
/** Until a graph has this many points it grows in from the right. */
const FRESH = 12;

export interface Line {
  values: number[];
  /** A token: "--accent", "--info", "--warn". */
  color: string;
}

const tokens = new Map<string, string>();
/** A colour token's value (canvas does not read CSS variables). */
function token(name: string): string {
  let value = tokens.get(name);
  if (!value) {
    value = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "#fff";
    tokens.set(name, value);
  }
  return value;
}

/**
 * Draws the lines over `max`, the newest point at the right edge. A graph
 * that has just begun spreads what it has over the width (from FRESH points
 * on), so it is never a stub in the corner; a full one holds HISTORY points.
 * `fill` shades under the first line.
 */
export function drawGraph(canvas: HTMLCanvasElement, lines: Line[], max: number, fill = true) {
  const box = canvas.getBoundingClientRect();
  if (box.width < 2 || box.height < 2) return;
  const dpr = window.devicePixelRatio || 1;
  const [w, h] = [Math.round(box.width * dpr), Math.round(box.height * dpr)];
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.clearRect(0, 0, w, h);
  const stroke = 1.5 * dpr;
  const top = Math.max(max, 1e-9);
  // Half a line in from the edges: the stroke is never cut.
  const y = (v: number) => h - stroke / 2 - Math.max(0, Math.min(1, v / top)) * (h - stroke);
  lines.forEach((line, index) => {
    const values = line.values.slice(-HISTORY);
    if (values.length < 2) return;
    const step = w / (Math.max(values.length, FRESH) - 1);
    const x = (i: number) => w - (values.length - 1 - i) * step;
    const trace = () => {
      ctx.beginPath();
      values.forEach((v, i) => (i ? ctx.lineTo(x(i), y(v)) : ctx.moveTo(x(i), y(v))));
    };
    const color = token(line.color);
    if (fill && index === 0) {
      trace();
      ctx.lineTo(w, h);
      ctx.lineTo(x(0), h);
      ctx.closePath();
      const shade = ctx.createLinearGradient(0, 0, 0, h);
      shade.addColorStop(0, color);
      shade.addColorStop(1, "transparent");
      ctx.globalAlpha = 0.22;
      ctx.fillStyle = shade;
      ctx.fill();
      ctx.globalAlpha = 1;
    }
    trace();
    ctx.lineWidth = stroke;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.strokeStyle = color;
    ctx.stroke();
  });
}
