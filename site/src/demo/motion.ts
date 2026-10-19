import { defineControls, knob, spring } from "knobs"

// Every motion value the stage demo uses, in one place. The dev server shows these as knobs (hover
// the stage for the ⚙ chip); production renders the defaults, or `?look=3` for the richer variant.
//
//   ?look=2  Restrained (default): springs a hair under critical, one faint sheen, no glow.
//   ?look=3  Richer: a little more overshoot, a brighter sheen and tint, slightly longer reveals.
export const motionControls = defineControls(
  "demo-motion",
  {
    badge: spring({ duration: 0.55, bounce: 0.14 }, { label: "Badge spring", group: "Fix lands" }),
    actions: spring({ duration: 0.5, bounce: 0.1 }, { label: "Button row spring", group: "Fix lands" }),
    rows: spring({ duration: 0.5, bounce: 0 }, { label: "Row padding spring", group: "Fix lands" }),
    stagger: knob(120, { min: 0, max: 300, step: 5, unit: "ms", label: "Badge → rows → buttons", group: "Fix lands" }),
    sheen: knob(0.16, { min: 0, max: 0.5, step: 0.01, label: "Sheen intensity", group: "Fix lands" }),
    sheenMs: knob(620, { min: 200, max: 1400, step: 10, unit: "ms", label: "Sheen duration", group: "Fix lands" }),
    tint: knob(0.14, { min: 0, max: 0.4, step: 0.01, label: "Diff applied tint", group: "Fix lands" }),

    wordMs: knob(240, { min: 80, max: 500, step: 10, unit: "ms", label: "Word reveal", group: "Text" }),
    wordRise: knob(3, { min: 0, max: 8, step: 0.5, unit: "px", label: "Word rise", group: "Text" }),
    wordBlur: knob(1.5, { min: 0, max: 4, step: 0.1, unit: "px", label: "Word blur", group: "Text" }),
    clauseMs: knob(280, { min: 120, max: 600, step: 10, unit: "ms", label: "Clause swap", group: "Text" }),
    streamFrom: knob(0.4, { min: 0, max: 1, step: 0.05, label: "Token start opacity", group: "Text" }),
    streamMs: knob(220, { min: 60, max: 500, step: 10, unit: "ms", label: "Token fade", group: "Text" }),
    enterMs: knob(260, { min: 120, max: 500, step: 10, unit: "ms", label: "Block enter", group: "Text" }),
    enterRise: knob(4, { min: 0, max: 10, step: 0.5, unit: "px", label: "Block rise", group: "Text" }),

    scrollMs: knob(420, { min: 0, max: 900, step: 10, unit: "ms", label: "Terminal scroll glide", group: "Stage" }),
    pin: spring({ duration: 0.34, bounce: 0.22 }, { label: "Pin drop spring", group: "Stage" }),
    rippleMs: knob(460, { min: 200, max: 900, step: 10, unit: "ms", label: "Click ripple", group: "Stage" }),
    rippleAlpha: knob(0.5, { min: 0, max: 1, step: 0.05, label: "Ripple opacity", group: "Stage" }),
    inkSheen: knob(0.5, { min: 0, max: 1, step: 0.05, label: "Wet ink sheen", group: "Stage" }),
  },
  {
    enabled: import.meta.env.DEV,
    defaultLabel: "Restrained",
    presets: {
      richer: {
        badge: { duration: 0.6, bounce: 0.24 },
        actions: { duration: 0.55, bounce: 0.18 },
        rows: { duration: 0.55, bounce: 0 },
        stagger: 140,
        sheen: 0.24,
        sheenMs: 720,
        tint: 0.2,
        wordMs: 260,
        wordRise: 4,
        wordBlur: 2,
        clauseMs: 320,
        pin: { duration: 0.36, bounce: 0.3 },
        inkSheen: 0.7,
      },
    },
    presetLabels: { richer: "Richer" },
  },
)

export type MotionValues = ReturnType<typeof motionControls.get>

const lookParam = typeof window === "undefined" ? null : new URLSearchParams(window.location.search).get("look")
if (lookParam === "3" && motionControls.enabled) motionControls.preset("richer")

const fixed: MotionValues | null =
  lookParam === "3" && !motionControls.enabled ? { ...motionControls.get(), ...motionControls.presets.richer } : null

// Reduced motion keeps every change visible but drops the decoration: no rise, blur, overshoot,
// sheen or ink gloss. The fix still glides, because the glide is the information.
const reduceQuery = typeof window === "undefined" ? null : window.matchMedia?.("(prefers-reduced-motion: reduce)")
let reducedCache: { from: MotionValues; to: MotionValues } | null = null
const reduce = (v: MotionValues): MotionValues => {
  if (reducedCache?.from === v) return reducedCache.to
  const to: MotionValues = {
    ...v,
    badge: { ...v.badge, bounce: 0 },
    actions: { ...v.actions, bounce: 0 },
    pin: { ...v.pin, bounce: 0 },
    wordRise: 0,
    wordBlur: 0,
    enterRise: 0,
    sheen: 0,
    inkSheen: 0,
  }
  reducedCache = { from: v, to }
  return to
}

// What the prerendered HTML used, so hydration starts from the same values.
export const motionServer = (): MotionValues => fixed ?? motionControls.get()

export const motion = (): MotionValues => {
  const v = fixed ?? motionControls.get()
  return reduceQuery?.matches ? reduce(v) : v
}

// Closed-form step response (0 → 1) of a SwiftUI-style spring at `ms` after it starts, so any frame
// renders identically in any order. ω = 2π / duration; ζ = 1 − bounce.
export function springAt(ms: number, s: { duration: number; bounce: number }) {
  if (ms <= 0) return 0
  const t = ms / 1000
  const w = (2 * Math.PI) / Math.max(0.01, s.duration)
  const z = Math.max(0.05, 1 - s.bounce)
  if (z >= 1) return 1 - (1 + w * t) * Math.exp(-w * t)
  const wd = w * Math.sqrt(1 - z * z)
  return 1 - Math.exp(-z * w * t) * (Math.cos(wd * t) + ((z * w) / wd) * Math.sin(wd * t))
}

// Minimum-jerk (quintic smootherstep): continuous velocity and acceleration into rest.
export const smoother = (p: number) => {
  const x = Math.min(1, Math.max(0, p))
  return x * x * x * (x * (x * 6 - 15) + 10)
}

export const easeOutCubic = (p: number) => 1 - Math.pow(1 - Math.min(1, Math.max(0, p)), 3)
