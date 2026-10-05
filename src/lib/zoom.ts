/** Zoom presets shared by the Loupe / Compare toolbar and the Develop toolbar + Navigator. */
export type ZoomPreset = "fit" | "fill" | 50 | 100 | 200 | 400;

export const ZOOM_PRESETS: { id: ZoomPreset; label: string; title: string }[] = [
  { id: "fit", label: "Fit", title: "Fit the whole photo" },
  { id: "fill", label: "Fill", title: "Fill the window (crops the long edge)" },
  { id: 50, label: "50%", title: "Zoom to 50%" },
  { id: 100, label: "100%", title: "Zoom to 100% (1:1)" },
  { id: 200, label: "200%", title: "Zoom to 200%" },
  { id: 400, label: "400%", title: "Zoom to 400%" },
];

// Space / click toggles Fit and the LAST zoom-in preset chosen (Lightroom), shared by Loupe, Compare and Develop for the session.
let lastZoomIn: Exclude<ZoomPreset, "fit"> = 100;
export const getLastZoomIn = () => lastZoomIn;
export function rememberZoom(p: ZoomPreset) {
  if (p !== "fit") lastZoomIn = p;
}
