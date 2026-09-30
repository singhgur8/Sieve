import { useEffect, useRef } from "react";
import type { Histogram as Hist } from "../../ipc";

/** RGB channels (additive blend) with a luma outline, from `RenderedPreview.histogram`. */
export function HistogramView({ h }: { h: Hist | null }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = ref.current;
    if (!c) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    const W = c.width;
    const H = c.height;
    ctx.clearRect(0, 0, W, H);
    if (!h) return;
    // sqrt-compress so a clipped spike does not flatten the rest; normalise on the 99th-ish max.
    const all = [h.red, h.green, h.blue, h.luma].flatMap((a) => [...a].sort((x, y) => y - x).slice(1, 2));
    const peak = Math.max(1, ...all);
    const y = (v: number) => H - Math.min(1, Math.sqrt(v / peak)) * (H - 2);
    const path = (bins: number[], close: boolean) => {
      ctx.beginPath();
      if (close) ctx.moveTo(0, H);
      bins.forEach((v, i) => (i === 0 && !close ? ctx.moveTo(0, y(v)) : ctx.lineTo((i / (bins.length - 1)) * W, y(v))));
      if (close) ctx.lineTo(W, H);
    };
    ctx.globalCompositeOperation = "lighter";
    (
      [
        [h.red, "rgba(255,40,40,0.55)"],
        [h.green, "rgba(40,255,40,0.55)"],
        [h.blue, "rgba(60,80,255,0.6)"],
      ] as const
    ).forEach(([bins, color]) => {
      path([...bins], true);
      ctx.fillStyle = color;
      ctx.fill();
    });
    ctx.globalCompositeOperation = "source-over";
    path([...h.luma], false);
    ctx.strokeStyle = "rgba(255,255,255,0.8)";
    ctx.lineWidth = 1;
    ctx.stroke();
  }, [h]);
  return <canvas ref={ref} width={256} height={96} data-testid="histogram" data-empty={h ? "false" : "true"} className="block h-24 w-full rounded bg-neutral-900" />;
}
