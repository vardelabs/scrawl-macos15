import { useEffect, useLayoutEffect, useState } from "react"

export type Platform =
  | { kind: "mac"; macos?: number }
  | { kind: "mac-intel" }
  | { kind: "windows" }
  | { kind: "linux" }
  | { kind: "mobile" }

type UAData = {
  platform?: string
  mobile?: boolean
  getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string; platformVersion?: string }>
}

const KINDS = ["mac", "mac-intel", "windows", "linux", "mobile"] as const

function uaData(): UAData | undefined {
  return (navigator as Navigator & { userAgentData?: UAData }).userAgentData
}

// Safari reports "Apple GPU" on every Mac, so it can't tell; Firefox and
// Chromium name the chip ("Apple M2", "Intel Iris", "AMD Radeon").
function macChip(): "arm" | "intel" | null {
  try {
    const gl = document.createElement("canvas").getContext("webgl")
    if (!gl) return null
    const read = (param: number) => String(gl.getParameter(param) ?? "")
    let renderer = read(gl.RENDERER)
    if (!/Apple M\d|Intel|AMD|Radeon|NVIDIA|GeForce/i.test(renderer)) {
      const ext = gl.getExtension("WEBGL_debug_renderer_info")
      if (ext) renderer = read(ext.UNMASKED_RENDERER_WEBGL)
    }
    gl.getExtension("WEBGL_lose_context")?.loseContext()
    if (/Apple M\d/.test(renderer)) return "arm"
    if (/Intel|AMD|Radeon|NVIDIA|GeForce/i.test(renderer)) return "intel"
  } catch {}
  return null
}

function override(): Platform | null {
  const value = new URLSearchParams(location.search).get("platform")
  return KINDS.find((k) => k === value) ? ({ kind: value } as Platform) : null
}

// Synchronous, so the first paint already shows the right button.
function guess(): Platform {
  const forced = override()
  if (forced) return forced
  const ua = navigator.userAgent
  const hint = uaData()?.platform ?? ""
  if (/iPhone|iPod|iPad|Android/.test(ua) || uaData()?.mobile || hint === "Android") return { kind: "mobile" }
  // iPadOS Safari asks for the desktop site with a Mac user agent.
  if (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1) return { kind: "mobile" }
  if (/Windows/.test(ua) || hint === "Windows") return { kind: "windows" }
  if (/Mac OS X|Macintosh/.test(ua) || hint === "macOS") return macChip() === "intel" ? { kind: "mac-intel" } : { kind: "mac" }
  return { kind: "linux" }
}

// Chromium can say exactly which chip and which macOS this is. Every Mac user
// agent string claims "Intel Mac OS X 10_15_7", so this is the only way to
// learn either from the browser.
async function refine(p: Platform): Promise<Platform> {
  const data = uaData()
  if ((p.kind !== "mac" && p.kind !== "mac-intel") || !data?.getHighEntropyValues || override()) return p
  try {
    const { architecture, platformVersion } = await data.getHighEntropyValues(["architecture", "platformVersion"])
    if (architecture === "x86") return { kind: "mac-intel" }
    const macos = parseInt(platformVersion ?? "", 10)
    if (architecture === "arm" || p.kind === "mac") return Number.isFinite(macos) ? { kind: "mac", macos } : { kind: "mac" }
  } catch {}
  return p
}

let first: Platform | undefined
let refined: Promise<Platform> | undefined

// The prerendered HTML shows the Mac download; the visitor's own platform
// replaces it before the first paint after hydration.
export function usePlatform(): Platform {
  const [platform, setPlatform] = useState<Platform>({ kind: "mac" })
  useLayoutEffect(() => {
    const p = (first ??= guess())
    setPlatform((prev) => (prev.kind === p.kind ? prev : p))
  }, [])
  useEffect(() => {
    let live = true
    ;(refined ??= refine(first ??= guess())).then((p) => {
      if (live) setPlatform((prev) => (JSON.stringify(prev) === JSON.stringify(p) ? prev : p))
    })
    return () => {
      live = false
    }
  }, [])
  return platform
}
