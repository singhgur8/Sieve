// Help & FAQ entries. Every statement here was checked against the code (App.tsx culling actions, repo::apply_suggestions,
// xmp::sidecar_path, KEEPER_RULES). Plain strings are what the search looks at; legends carry their own labels.
import type { ReactNode } from "react";
import { Anchor, CheckCircle2, CloudOff, CloudUpload, FileCheck, Flag, ImageOff, Layers, Loader2, Unplug, X, AlertTriangle } from "lucide-react";
import { Stars } from "../components/Cell";
import { DISPLAY_ORDER, KEEPER_RULES } from "../components/KeeperRule";
import { ALL_TAGS, LABEL_COLOR, TAG_SHORT, TAG_STYLE, tagName } from "./format";
import { TAG_MEANING } from "./cull";

export interface LegendItem {
  icon: ReactNode;
  label: string;
  text: string;
}

export type Block = { p: string } | { ul: string[] } | { legend: LegendItem[]; title: string } | { shortcuts: true };

export interface HelpEntry {
  id: string;
  title: string;
  /** Extra search words. */
  keywords?: string;
  blocks: Block[];
}

const badge = "flex items-center gap-0.5 rounded px-1 text-[10px]";

const LEGEND_FLAGS: LegendItem[] = [
  { icon: <Flag className="size-3.5 fill-green-500 text-green-500" />, label: "Pick flag", text: "You (or Auto) picked this photo." },
  { icon: <X className="size-4 text-red-500" strokeWidth={3} />, label: "Reject flag", text: "Rejected. The picture is dimmed. Nothing is deleted." },
  { icon: <Stars n={3} />, label: "Stars", text: "Your rating, 0 to 5. Click a star to rate; click the same star to clear." },
  {
    icon: (
      <span className="flex gap-0.5">
        {["red", "yellow", "green", "blue", "purple"].map((l) => (
          <span key={l} className={`size-2.5 rounded-full ${LABEL_COLOR[l]}`} />
        ))}
      </span>
    ),
    label: "Color label",
    text: "Colored dot (keys 6 to 9 set red, yellow, green, blue).",
  },
];

const LEGEND_TAGS: LegendItem[] = ALL_TAGS.map((t) => ({
  icon: <span className={`rounded px-1 text-[9px] font-semibold leading-4 ${TAG_STYLE[t]}`}>{TAG_SHORT[t]}</span>,
  label: tagName(t).replace(/^./, (c) => c.toUpperCase()),
  text: TAG_MEANING[t], // one source with the hover text of the thumbnails
}));

const LEGEND_STRIPS: LegendItem[] = [
  {
    icon: <span className="block h-3 w-14 bg-red-950/85" />,
    label: "Rejected strip",
    text: "Rejected, and by whom (you or Auto), with Sieve's reason",
  },
  {
    icon: <span className="block h-3 w-14 border-t border-dashed border-red-400/60 bg-sky-950/85" />,
    label: "Suggestion strip",
    text: "Sieve suggests rejecting this photo. Nothing has changed until you press X or Apply suggestions",
  },
];

const LEGEND_BADGES: LegendItem[] = [
  {
    icon: (
      <span className={`${badge} bg-green-900 text-green-200`}>
        <Layers className="size-3" />★
      </span>
    ),
    label: "Best of burst",
    text: "Sieve's choice of the best frame of a burst (shots taken within about a second or two of each other).",
  },
  {
    icon: (
      <span className={`${badge} bg-neutral-800/90 text-neutral-300`}>
        <Layers className="size-3" />
      </span>
    ),
    label: "Burst frame",
    text: "Another frame of a burst. \"Collapse bursts\" in the filter bar shows only the best frame of each burst.",
  },
  {
    icon: (
      <span className={`${badge} bg-emerald-950/90 text-emerald-200`}>S1</span>
    ),
    label: "Scene",
    text: "Photos in the same lighting are grouped into scenes (Edit step).",
  },
  {
    icon: (
      <span className={`${badge} bg-amber-800 text-amber-100`}>
        <Anchor className="size-3" />S1
      </span>
    ),
    label: "Scene anchor",
    text: "The photo whose edit is copied to the rest of its scene.",
  },
  { icon: <span className="rounded bg-neutral-800/90 px-1 text-[10px] font-semibold text-neutral-200">+JPG</span>, label: "+JPG", text: "A camera JPEG next to this RAW was paired with it at import." },
  {
    icon: (
      <span className={`${badge} bg-red-900 font-semibold text-red-100`}>
        <ImageOff className="size-3" />
        Unreadable
      </span>
    ),
    label: "Unreadable",
    text: "The file could not be decoded. It may be damaged or still copying.",
  },
  { icon: <FileCheck className="size-3.5 text-emerald-500" />, label: "Saved check", text: "Loupe only: the sidecar is up to date (Saved to the .xmp next to the original)." },
  { icon: <CloudUpload className="size-3.5 text-amber-400" />, label: "Not saved yet", text: "This photo's changes have not been written to its XMP sidecar yet." },
  { icon: <AlertTriangle className="size-3.5 text-red-400" />, label: "Sidecar error", text: "The sidecar could not be written. Hover for the reason." },
  {
    icon: (
      <span className={`${badge} bg-amber-800 font-semibold text-amber-100`}>
        <Unplug className="size-3" />
        Missing
      </span>
    ),
    label: "Missing",
    text: "The original file cannot be found (moved, or the drive is disconnected).",
  },
];

const LEGEND_STATUS: LegendItem[] = [
  { icon: <CheckCircle2 className="size-3.5 text-emerald-400" />, label: "Saved", text: "Everything is written to the sidecars." },
  { icon: <Loader2 className="size-3.5 text-amber-300" />, label: "Saving", text: "Changes are being written." },
  { icon: <CloudOff className="size-3.5 text-neutral-400" />, label: "Auto-save off", text: "Changes stay in the catalog until you press Save (Cmd+S)." },
  { icon: <AlertTriangle className="size-3.5 text-red-400" />, label: "Errors", text: "Some sidecars could not be written. Click the pill for the list and Retry." },
];

export const HELP: HelpEntry[] = [
  {
    id: "culling",
    title: "Culling workflow",
    keywords: "pick reject flag stars rating label undo burst compare keys",
    blocks: [
      { p: "Go through the photos one by one and flag each one. Nothing is deleted: rejected photos are only dimmed and flagged." },
      {
        ul: [
          "Z picks, X rejects, U clears the flag. P also picks.",
          "0 to 5 sets the stars. 6 to 9 set a color label.",
          "Shift+Z and Shift+X flag and then move to the next photo.",
          "Left / Right move between photos. Space opens the Loupe. In the Loupe, Compare and Develop, Space (or a click in the Loupe) zooms between Fit and 1:1 at the point under the cursor, and the zoom and position stay while you step with Left / Right. The zoom buttons add Fill, 50%, 200% and 400%; drag to pan.",
          "C opens Compare: two photos side by side. Tab switches the active pane. K makes the active frame the best of its burst and picks it.",
          "Cmd+Z undoes the last culling change.",
        ],
      },
      { p: "Bursts: photos shot close together are grouped, and Sieve marks the best frame of each burst. Use Collapse bursts in the filter bar to see only the best frame of each burst, or Cmd+Shift+B to select a whole burst." },
      { p: "After Analyze, photos get tags (blink, missed focus, ...). Click a tag in the filter bar to show it, click again to hide it. See the Icons entry for what each one means." },
    ],
  },
  {
    id: "pick-best-n",
    title: "Pick the best N",
    keywords: "target count deliver delivery set best pick alternatives people couple important second look swap add moment candid group detail",
    blocks: [
      { p: "Instead of flagging every photo, tell Sieve how many you want to deliver (for example 800). It picks the delivery set, and you review only the picks and the borderline photos. The count is a guideline, not a limit. Open it with \"Pick the best N\" in the Cull toolbar." },
      {
        ul: [
          "Pick: enter the count and the shoot type (remembered per shoot type) and run it. The summary shows how many were picked per kind of shot (couple, group, detail, candid), how many are Not sure and how many are Set aside.",
          "People: Sieve shows who it thinks the couple is (confirm or change who), then asks \"Is this person important?\" for other recurring people. Y means yes, N means no, the arrow keys move between people. Then \"Re-run with these people\". A re-run keeps the picks you have reviewed and every keep, set-aside and swap you made.",
          "Review picks: one picked photo at a time with its whole moment in a strip (the other picks, then the alternatives), each with a reason. It opens at the first pick you have not looked at. Left / Right move, Z or Enter keeps, X sets it aside (it is not rejected), Tab (or Up / Down) cycles the alternatives, S swaps the shown alternative in, A adds it too, Space zooms both photos to 100%. Shift+Right accepts the rest of the moment and goes to the next one; ] and [ jump to the next / previous photo you have not looked at.",
          "Second look: four piles. Not sure (the default) is one photo at a time, by moment, with the similar photo that is already kept and a strip of everything kept from the moment. Space or Enter skips (it moves on by itself), Shift+Space skips the rest of the moment, S keeps this one instead of the kept one, A adds both, X sets it aside. Similar to a kept photo is one row per moment next to the kept photo. Weaker frames and Defects are plain grids, not part of the pass. Click a photo to zoom both.",
          "Cmd+Z undoes every change made here. Nothing is deleted. Keeps, swaps, adds and set-asides write flags (and XMP) as you make them; Cmd+Z undoes them. Apply flags the rest: the picks, and (if you leave the box ticked) rejects the photos Sieve is confident are defects. Undo it from the Apply result, or with Cmd+Z in the grid.",
          "Apply flags writes Picked to the delivery set (flags and XMP sidecars). Photos you flagged or starred yourself are never changed. Afterwards Sieve offers to set the keeper rule to \"Picks, 1★+\" so the picks become the keepers that Edit and Export work on.",
          "In the grid, Show in grid (or the Best N chip in the filter bar) lists the picks, or the Not sure and Set aside photos.",
        ],
      },
      { shortcuts: true },
    ],
  },
  {
    id: "auto-advance",
    title: "Auto-advance and Caps Lock",
    keywords: "auto advance next photo caps lock",
    blocks: [
      { p: "With Auto-advance on, Sieve moves to the next photo after you flag, rate or label one photo." },
      {
        ul: [
          "It only moves when exactly one photo is targeted. With several photos selected, it stays put.",
          "In Compare it moves to the next candidate.",
          "Shift+Z and Shift+X always advance, even with Auto-advance off.",
          "Caps Lock works as Auto-advance while it is on (like Lightroom).",
        ],
      },
    ],
  },
  {
    id: "apply-suggestions",
    title: "Apply suggestions (Auto)",
    keywords: "auto analyze suggested rejects review undo",
    blocks: [
      { p: "Analyze scores every photo. Apply suggestions (the \"Suggestions: … — Apply…\" button at the right of the Cull summary, or More > Apply suggestions…) copies Sieve's suggested flag and star rating onto the photos, so you start from a first pass instead of a blank slate." },
      {
        ul: [
          "Scope: the selected photos or everything in view.",
          "By default it skips photos you already flagged or rated. Untick that to overwrite them.",
          "Photos that were not analyzed yet are skipped.",
          "Flags it sets are marked as automatic, so they can be told apart from yours. It does not change color labels or edits. Like any flag or rating, the result is written to the XMP sidecars (right away with auto-save on, or when you press Cmd+S).",
          "Undo brings the old flags and ratings back (the Undo button in the message, or Cmd+Z).",
        ],
      },
      { p: "To review its rejects: click Rejected in the Cull summary or the filter bar. Each rejected thumbnail says who rejected it (you or Auto) and why (for example Eyes closed); hover any badge for its meaning. Or click N auto in the Cull summary to see only Auto's rejects. Press U on any photo you want to keep, or Z to pick it." },
    ],
  },
  {
    id: "keepers",
    title: "Keepers",
    keywords: "keeper rule edit export not rejected best of burst",
    blocks: [
      { p: "Keepers are the photos that move on to the Edit and Export steps. By default that is everything you have not rejected." },
      { p: "To change the rule, use \"Change keeper rule\" in the Cull summary bar (under the toolbar), the Keepers menu in the Edit plan, or the Export dialog. The choices:" },
      { ul: DISPLAY_ORDER.map((i) => `${KEEPER_RULES[i].label}: ${KEEPER_RULES[i].long}.`) },
      { p: "The \"Continue to Edit\" button shows how many keepers the current rule gives." },
    ],
  },
  {
    id: "icons",
    title: "Icons and badges",
    keywords: "legend meaning icon badge tag flag star label burst scene sidecar missing",
    blocks: [
      { legend: LEGEND_FLAGS, title: "Flags, stars and labels" },
      { legend: LEGEND_TAGS, title: "Culling tags" },
      { legend: LEGEND_STRIPS, title: "Strips on thumbnails" },
      { legend: LEGEND_BADGES, title: "Badges on thumbnails" },
      { legend: LEGEND_STATUS, title: "Save status (top bar)" },
    ],
  },
  {
    id: "saved",
    title: "Where your work is saved",
    keywords: "xmp sidecar auto-save catalog photos copy autosave",
    blocks: [
      { p: "Sieve never copies or changes your photos. Your work is saved in .xmp sidecar files next to the originals: DSC0001.ARW gets DSC0001.xmp. (For JPEG, TIFF, PNG and HEIC the sidecar is DSC0001.jpg.xmp.)" },
      {
        ul: [
          "Saved in the sidecar: stars, pick / reject flags, color labels, Sieve's tags and your edits. Fields written by other apps are kept.",
          "Auto-save writes a sidecar a moment after each change. The pill in the top bar shows Saved, Saving, Auto-save off or errors.",
          "Cmd+S saves the selection right away. With auto-save off, changes wait in the catalog until you save.",
          "The catalog is a database in the app's data folder. It holds your projects, thumbnails and analysis so the app opens fast. Exports go where you choose.",
        ],
      },
    ],
  },
  {
    id: "lightroom",
    title: "Lightroom round trip",
    keywords: "adobe lightroom classic xmp pick flag sync metadata read save bridge",
    blocks: [
      { p: "Sieve and Lightroom Classic share the sidecars." },
      {
        ul: [
          "Flags are stored as xmpDM:pick / xmpDM:good. Lightroom Classic reads them from version 13.2. Older versions show the stars and labels but not the flags.",
          "New photos: when you import a folder into Lightroom, it reads the sidecars, so your culling shows up.",
          "Photos already in a Lightroom catalog: select them, then Metadata > Read Metadata from Files.",
          "Going back: Lightroom writes its changes to the sidecars with Cmd+S (Metadata > Save Metadata to Files), or automatically if you turn on \"Automatically write changes into XMP\" in Catalog Settings > Metadata. If Lightroom shows the \"metadata changed externally\" badge on a thumbnail, click it and choose Import Settings from Disk.",
          "Sieve re-reads sidecars that another app changed when a project opens and when the window regains focus.",
          "Do not save from both apps at the same moment. Save in one, then switch to the other.",
        ],
      },
    ],
  },
  {
    id: "capture-time",
    title: "Capture time and photo info",
    keywords: "date time clock camera sync shift hours timezone daylight saving two cameras edit capture time metadata exif info panel",
    blocks: [
      { p: "Open Photo info (the Info button, or Cmd+I) to see the selected photo's capture date and time, camera, lens, exposure, size, GPS and sidecar. It follows your selection. The Metadata filter in the filter bar is different: it filters the whole gallery by camera, lens, ISO and so on." },
      { p: "Camera clock wrong? Edit capture time (Photo info, More menu or Cmd+Shift+T) works like Lightroom's Edit Capture Time:" },
      {
        ul: [
          "Shift by hours / minutes: moves the selected photos by the same amount (for example -1 h after forgetting daylight saving).",
          "Set exact time: gives the active photo its true time; the other selected photos move by the same amount.",
          "Sync two cameras: pick one frame from each camera that was taken at the same moment. Every photo of the second camera moves so its frame matches the first camera. A preview shows before and after.",
          "Revert to original: back to the time stored in the file.",
        ],
      },
      { p: "Sorting, bursts and scenes use the corrected time, so a mixed two-camera shoot falls into the right order. The original files are never changed; the correction is saved in the XMP sidecars (Lightroom corrections are picked up from them). Undo from the message, or Revert to original. The original time is shown next to the corrected one." },
    ],
  },
  {
    id: "reject-strictness",
    title: "Reject strictness",
    keywords: "conservative balanced aggressive suggestions reject auto cull strict",
    blocks: [
      { p: "In the Cull step, Reject strictness sets how readily Sieve suggests rejecting a photo, per project. Conservative: only unusable frames (nothing in focus, far too dark or blown out, or several defects at once); closed eyes and burst duplicates are never rejected. Balanced (default): also missed focus or motion blur on the main subject, closed eyes on the main subject, and burst frames clearly worse than the best one. Aggressive: also any closed eyes or soft focus, and weaker burst frames even when the difference is small; expect some keepers among the suggestions." },
      { p: "Changing it re-evaluates the suggestions (the count next to it updates). Nothing is rejected until you press Apply suggestions, and you can undo that." },
    ],
  },
  {
    id: "baseline-edit",
    title: "Baseline edit",
    keywords: "baseline preset anchor edit the rest whole shoot lightroom finish auto light white balance undo needs a look flagged",
    blocks: [
      { p: "The fast way to get a good starting edit for a whole shoot, then finish it in Lightroom. In the Edit step choose Start baseline edit (Cmd+Alt+B)." },
      {
        ul: [
          "1. Preset: pick a look from your library. Each tile is the anchor photo rendered with that preset. Search by name or filter by group, or choose No preset.",
          "2. Anchor photo: the one photo you adjust by hand. Pick a good keeper in typical light.",
          "3. Adjust it in Develop, light and white balance first. The Baseline bar shows how your photo differs from its own Auto, for example +0.3 EV, warmer than Auto. Press Edit the rest when you are happy.",
          "4. Edit the rest: choose keepers, all photos or the selection, and whether to skip or replace photos you already edited. The preview shows before and after for about twelve photos across your scenes. Edit applies it with progress, a Stop button and one Undo for the whole batch.",
          "5. Finish in Lightroom: make sure the sidecars are saved, then in Lightroom select the photos and choose Metadata > Read Metadata from Files (a new import reads the sidecars by itself).",
          "The preset's look (colors, curves, grain and so on) is copied as it is. Each photo keeps its own exposure and white balance, shifted by the same amount you shifted the anchor from its Auto, so a dark church and a sunny park both come out well exposed. Frames of one burst or scene get matching values.",
          "Photos that are dark on purpose, silhouettes, or where Auto failed are edited too but marked Needs a look. Use Show the photos that need a look to list them with the reason.",
          "Crop, straighten and masks are never touched. Scene by scene editing (Apply to scene) is still there in the Plan for refinements.",
        ],
      },
    ],
  },
  {
    id: "editing",
    title: "Editing basics",
    keywords: "develop sliders exposure scene apply copy paste sync auto edit export type value arrow keys blue dot preset preview highlight",
    blocks: [
      {
        ul: [
          "Press D to open Develop. Sliders (Exposure, Contrast, Highlights, Shadows, White balance, ...) are non-destructive and saved to the sidecar. Double-click a slider's name to reset it.",
          "In the Edit step, the quickest start is Baseline edit (one preset + one adjusted photo for the whole shoot). To refine scene by scene: keepers are grouped into scenes by lighting; edit one photo per scene, then Apply to scene copies that edit to the others, matched for exposure and white balance.",
          "Auto edit (my style) edits a scene's photo with your learned style. Review it, then apply.",
          "Copy, Paste and Sync move settings between photos. Cmd+C (or Ctrl+C) copies every setting, Cmd+V (Ctrl+V) pastes them as one undoable step. Cmd+Shift+C picks which settings to copy.",
          "Type a slider value: click the number, type, Enter applies, Esc cancels, Tab moves to the next slider. Hover a slider and press Up / Down to nudge it (Shift = x10, Alt = fine). Left / Right still move between photos unless a slider has focus.",
          "Auto (top of Basic) sets tone and white balance from the photo itself, on any photo. It is not Auto edit (my style), which applies your learned style to a scene.",
          "A blue dot next to a panel's name (Basic, Detail, Effects, ...) means it was changed. Press and hold the dot to see the photo without those changes; release to come back. Nothing is saved.",
          "The preset the photo carries is highlighted in the Presets list. Hover a preset to preview it on the photo; click to apply it.",
          "Backslash shows before / after. Cmd+Shift+R resets all adjustments.",
          "Export (Cmd+Shift+E) renders JPEG, TIFF or PNG with your preset. The originals stay untouched.",
        ],
      },
    ],
  },
  {
    id: "upright",
    title: "Crop, straighten and Upright",
    keywords: "crop straighten angle horizon level vertical perspective keystone transform guided auto upright lines",
    blocks: [
      {
        ul: [
          "Press R to crop. Drag the corners, edges or the frame; drag outside the frame to rotate. A locks the aspect ratio, X swaps landscape / portrait, O cycles the guides. Enter applies, Esc cancels.",
          "Auto (in the crop panel) straightens the photo: it finds the horizon or the vertical lines and sets the Angle for you. Shift+double-click the Angle slider does the same. If it finds no straight lines it tells you; draw the angle yourself with the Angle tool.",
          "Constrain to image keeps the crop inside the straightened photo. Turn it off to crop past the edge: the empty corners are filled white, as in Lightroom.",
          "Transform (right panel, below Detail) fixes perspective. Upright Level levels the horizon, Vertical also straightens converging verticals, Full corrects horizontal perspective too, and Auto picks a balanced correction. One click, one undo step.",
          "Guided (Shift+T) lets you draw 2 to 4 lines along things that should be straight, such as door frames or walls. Sieve corrects the photo as soon as there are two. Drag a line's ends to adjust it, click the x to delete it, Esc to finish.",
          "The sliders below (Vertical, Horizontal, Rotate, Aspect, Scale, Offset) adjust by hand. Constrain Crop keeps the crop inside the corrected image. The blue dot shows the transform was changed; hold it to see the photo without it.",
          "Transform is written to the sidecar so Lightroom shows the same Upright mode and sliders. A photo you already straightened with Upright in Lightroom keeps Lightroom's own correction.",
        ],
      },
    ],
  },
  {
    id: "shortcuts",
    title: "Keyboard shortcuts",
    keywords: "keys chords cheat sheet",
    blocks: [{ p: "Press ? at any time for the full list of shortcuts, grouped by where they apply." }, { shortcuts: true }],
  },
];

function blockText(b: Block): string {
  if ("p" in b) return b.p;
  if ("ul" in b) return b.ul.join(" ");
  if ("legend" in b) return `${b.title} ${b.legend.map((l) => `${l.label} ${l.text}`).join(" ")}`;
  return "";
}

const norm = (s: string) => s.toLowerCase().replace(/[-_]/g, " ");

/** Lower-cased searchable text per entry (hyphens read as spaces, so "auto advance" finds "Auto-advance"). */
export const HELP_TEXT: Record<string, string> = Object.fromEntries(HELP.map((e) => [e.id, norm(`${e.title} ${e.keywords ?? ""} ${e.blocks.map(blockText).join(" ")}`)]));

export function searchHelp(query: string): HelpEntry[] {
  const words = norm(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return HELP;
  const title = (e: HelpEntry) => words.every((w) => norm(e.title).includes(w));
  const hits = HELP.filter((e) => words.every((w) => HELP_TEXT[e.id].includes(w)));
  return [...hits.filter(title), ...hits.filter((e) => !title(e))];
}
