import { type RefObject, useEffect, useState, useSyncExternalStore } from "react"

// Runs `fn` once the page has loaded and the main thread is idle, so optional
// work (the 3D brush, the wet-ink shader, narration audio) never competes with
// the first paint. Returns a cancel function for effect cleanup.
export function afterLoad(fn: () => void, timeout = 3000): () => void {
  let cancelled = false
  let idle: number | undefined
  const call = () => {
    if (!cancelled) fn()
  }
  const schedule = () => {
    idle = typeof requestIdleCallback === "function" ? requestIdleCallback(call, { timeout }) : window.setTimeout(call, 200)
  }
  if (document.readyState === "complete") schedule()
  else window.addEventListener("load", schedule, { once: true })
  return () => {
    cancelled = true
    window.removeEventListener("load", schedule)
    if (idle !== undefined) (typeof cancelIdleCallback === "function" ? cancelIdleCallback : window.clearTimeout)(idle)
  }
}

const noSubscribe = () => () => {}

// False in the prerendered HTML and while React hydrates it, then true, so
// browser-only output (the demo's glass filter) doesn't break hydration.
export const useHydrated = () => useSyncExternalStore(noSubscribe, () => true, () => false)

// False until afterLoad fires and, given a ref, until that element is within
// 200 px of the viewport. Each caller renders in its own idle task,
// and parts far down the page don't render until someone scrolls to them.
export function useAfterLoad(near?: RefObject<Element | null>): boolean {
  const [ready, setReady] = useState(false)
  useEffect(() => {
    let io: IntersectionObserver | undefined
    const cancel = afterLoad(() => {
      const el = near?.current
      if (!el) return setReady(true)
      io = new IntersectionObserver(
        (entries) => {
          if (!entries.some((e) => e.isIntersecting)) return
          io?.disconnect()
          setReady(true)
        },
        { rootMargin: "200px 0px" },
      )
      io.observe(el)
    })
    return () => {
      cancel()
      io?.disconnect()
    }
  }, [near])
  return ready
}
