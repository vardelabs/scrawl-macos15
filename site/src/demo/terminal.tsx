import { memo, type ReactNode, type RefObject, useLayoutEffect, useRef, useState } from "react"
import { TrafficLights } from "./apps"
import { AGENT, type Mode, type Script, summaryText, T } from "./script"
import { motion, smoother } from "./motion"
import { ContactSheet, hunkDelay, terminalActive } from "./stage-parts"

// OpenCode's TUI (packages/tui) inside Ghostty, using the v2 "opencode" dark theme tokens:
// background #0a0a0a / raised #141414 / prompt #1e1e1e, text #eeeeee / muted #808080,
// Build agent blue #5c9cf5, paste extmarks on warning #f5a742.

const LOGO_LEFT = ["                   ", "█▀▀█ █▀▀█ █▀▀█ █▀▀▄", "█__█ █__█ █^^^ █__█", "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀~~▀"]
const LOGO_RIGHT = ["             ▄     ", "█▀▀▀ █▀▀█ █▀▀█ █▀▀█", "█___ █__█ █__█ █^^^", "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀"]

// Ghostty draws block elements as exact cell sprites, so render them as cells rather than font glyphs.
const CELL: Record<string, string> = { "█": "full", "▀": "top", "▄": "bottom", _: "sh", "^": "top-sh", "~": "sh-top" }

function LogoLine({ line, cls }: { line: string; cls: string }) {
  return (
    <span className={`lg ${cls}`}>
      {Array.from(line).map((c, i) => (
        <i key={i} className={CELL[c] ?? ""} />
      ))}
    </span>
  )
}

const Logo = memo(function Logo() {
  return (
    <div className="tui-logo" role="img" aria-label="opencode">
      {LOGO_LEFT.map((l, i) => (
        <div key={i}>
          <LogoLine line={l} cls="lg-open" />
          <i />
          <LogoLine line={LOGO_RIGHT[i]} cls="lg-code" />
        </div>
      ))}
    </div>
  )
})

function Cursor({ focused }: { focused: boolean }) {
  return <i className={`tcur${focused ? " on" : ""}`} />
}

// createFrames({ style: "blocks" }) from packages/tui/src/ui/spinner.ts: an 8-cell bidirectional
// scanner at 40ms per frame, holding 9 frames at the end and 30 at the start.
const TRAIL = [1, 0.82, 0.66, 0.52, 0.4, 0.32]

const Spinner = memo(function Spinner({ frame }: { frame: number }) {
  const f = frame % 54
  const forward = f < 8 || (f >= 24 && f < 54)
  const pos = f < 8 ? f : f < 17 ? 7 : f < 24 ? 6 - (f - 17) : 0
  const held = f >= 8 && f < 17 ? f - 7 : f >= 24 ? f - 23 : 0
  return (
    <span className="tspin" aria-hidden>
      {Array.from({ length: 8 }, (_, i) => {
        const d = forward ? pos - i : i - pos
        const trail = d >= 0 && d < TRAIL.length && d < (held ? Math.max(1, TRAIL.length - held) : TRAIL.length)
        return (
          <i key={i} style={{ opacity: trail ? TRAIL[d] : 0.36 }}>
            {trail ? "■" : "⬝"}
          </i>
        )
      })}
    </span>
  )
})

const BRAILLE = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

const Braille = memo(function Braille({ frame }: { frame: number }) {
  return <span className="tbraille">{BRAILLE[frame % BRAILLE.length]}</span>
})

// Realistic token-burst stream for the agent's final summary (starts at t = 20180, finishes before T.done = 21400).
const STREAM_START = T.fix[0] + 180
const STREAM_TOKENS: { text: string; code?: boolean; dt: number }[] = [
  { text: "The ", dt: 0 },
  { text: "Live", code: true, dt: 55 },
  { text: " badge", dt: 110 },
  { text: " now sits", dt: 175 },
  { text: " in the", dt: 225 },
  { text: " title row's", dt: 295 },
  { text: " flex flow,", dt: 365 },
  { text: " rows", dt: 455 },
  { text: " are back", dt: 510 },
  { text: " to ", dt: 555 },
  { text: "py-2.5", code: true, dt: 615 },
  { text: ",", dt: 660 },
  { text: " and the", dt: 730 },
  { text: " action row", dt: 795 },
  { text: " lost its", dt: 865 },
  { text: " stray ", dt: 920 },
  { text: "14px", dt: 980 },
  { text: " top padding.", dt: 1050 },
]

function Prompt({
  still,
  mode,
  pasteStage,
  submitted,
  done,
  focused,
  running,
  spinFrame,
}: {
  still?: boolean
  mode: Mode
  pasteStage: number
  submitted: boolean
  done: boolean
  focused: boolean
  running: boolean
  spinFrame: number
}) {
  // The pasted lines are laid out with the chip and fade in one by one, so the prompt grows once,
  // eased, instead of stepping a line at a time.
  const lines = pasteStage >= 1 ? summaryText(mode, true) : null
  return (
    <div className="tp">
      <div className="tp-box">
        <span className="tp-bar" />
        <div className="tp-inner">
          <AutoHeight watch={pasteStage > 0}>
          <div className="tp-text">
            {pasteStage >= 1 ? (
              <>
                <span className={`tx-paste${still ? "" : " enter-pop"}`}>[Image 1]</span>{" "}
                {lines &&
                  lines.map((line, idx) => {
                    const show = pasteStage >= idx + 2
                    return (
                      <span key={idx} className={`tp-paste-line${show ? " on" : ""}`}>
                        {idx > 0 ? "\n" : ""}
                        {line}
                      </span>
                    )
                  })}
                <Cursor focused={focused} />
              </>
            ) : (
              <span className="tp-ph">
                <Cursor focused={focused} />
                {submitted ? "" : 'Ask anything… "Fix a TODO in the codebase"'}
              </span>
            )}
          </div>
          </AutoHeight>
          <div className="tp-meta">
            <span className="tx-build">Build</span> <span className="tx-mute">·</span> Claude Opus 5.5 <span className="tx-mute">anthropic</span>
          </div>
        </div>
      </div>
      <div className="tp-foot-edge">
        <span className="tp-cap" />
        <span className="tp-half" />
      </div>
      <div className="tp-footer">
        {running ? (
          <span className="tp-status">
            <Spinner frame={spinFrame} />
            <span>
              esc <span className="tx-mute">interrupt</span>
            </span>
          </span>
        ) : (
          <span className="tx-mute">~/code/pulse:main</span>
        )}
        <span className="tp-keys">
          {done ? (
            <span className="tx-mute">18.4K (9%) · $0.11</span>
          ) : !submitted ? (
            <span>
              tab <span className="tx-mute">agents</span>
            </span>
          ) : null}
          <span>
            ctrl+p <span className="tx-mute">commands</span>
          </span>
        </span>
      </div>
    </div>
  )
}

// Eases its height to its content's whenever `watch` changes (measured synchronously, so the
// first frame of the change is already part of the glide).
function AutoHeight({ watch, children }: { watch: unknown; children: ReactNode }) {
  const inner = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState<number | null>(null)
  useLayoutEffect(() => {
    if (inner.current) setHeight(inner.current.offsetHeight)
  }, [watch])
  return (
    <div className="auto-height" style={{ height: height ?? undefined }}>
      <div ref={inner}>{children}</div>
    </div>
  )
}

type Tok = [string, string?]
const tsx = (src: string): Tok[] => {
  const out: Tok[] = []
  const re = /(<\/?|\/?>)|([A-Za-z][\w.]*)(?==)|("[^"]*")|(=)|(\b[A-Z]\w*\b)|(\b[a-z]\w*\b(?=[\s>]))|([^<>="A-Za-z]+|[A-Za-z]+)/g
  let m: RegExpExecArray | null
  let afterOpen = false
  while ((m = re.exec(src))) {
    if (m[1]) {
      out.push([m[1], "sx-op"])
      afterOpen = m[1].startsWith("<")
      continue
    }
    if (m[2]) out.push([m[2], "sx-attr"])
    else if (m[3]) out.push([m[3], "sx-str"])
    else if (m[4]) out.push([m[4], "sx-op"])
    else if (afterOpen && (m[5] || m[6])) out.push([m[5] || m[6], "sx-tag"])
    else out.push([m[0]])
    afterOpen = false
  }
  return out
}

const DIFF: [number, " " | "-" | "+", Tok[]][] = (
  [
    [13, " ", `<h3 className="title">Deploy Activity</h3>`],
    [14, "-", `<Badge className="absolute -left-14 -top-2">`],
    [14, "+", `<Badge className="ml-2.5">`],
    [31, "-", `<ul className="space-y-0 py-px">`],
    [31, "+", `<ul className="divide-y [&>li]:py-2.5">`],
    [44, "-", `<div className="h-8 overflow-hidden pt-3.5">`],
    [44, "+", `<div className="flex h-8 justify-end gap-2">`],
  ] as const
).map(([n, sign, code]) => [n, sign, tsx(code)])

// Each + line takes a brief green tint as its hunk lands on the stage (badge, rows, then buttons),
// so the diff line and the element it moves read as cause and effect. Instant attack, convex decay.
const HUNK_OF_LINE = [-1, -1, 0, -1, 1, -1, 2]

const Diff = memo(function Diff({ fixAge, tint }: { fixAge: number; tint: number }) {
  return (
    <div className="tdiff">
      {DIFF.map(([n, sign, tokens], i) => {
        const hunk = HUNK_OF_LINE[i]
        const age = hunk >= 0 && fixAge >= 0 ? fixAge - hunkDelay(hunk) : -1
        const glow = age < 0 ? 0 : age < 90 ? age / 90 : Math.pow(Math.max(0, 1 - (age - 90) / 900), 2)
        const a = tint * glow
        return (
          <div
            key={i}
            className={`td-line${sign === "+" ? " add" : sign === "-" ? " del" : ""}`}
            style={{
              animationDelay: `${i * 34}ms`,
              boxShadow: a > 0.002 ? `inset 0 0 0 999px rgba(127, 216, 143, ${a.toFixed(3)})` : undefined,
            }}
          >
            <span className="td-num">{n}</span>
            <span className="td-sign">{sign === " " ? " " : sign}</span>
            <span className="td-code">
              {tokens.map(([s, c], j) =>
                c ? (
                  <span key={j} className={c}>
                    {s}
                  </span>
                ) : (
                  s
                ),
              )}
            </span>
          </div>
        )
      })}
    </div>
  )
})

// Tokens arrive at their burst times and fade from a dim start to full text colour; the caret rides
// the leading edge. Every token is laid out from the start (pending ones are transparent), so an
// arriving word never rewraps the line before it.
function StreamedResponse({ streamElapsed, done, from, fadeMs }: { streamElapsed: number; done: boolean; from: number; fadeMs: number }) {
  let firstPending = STREAM_TOKENS.length
  for (let i = 0; i < STREAM_TOKENS.length; i++) {
    if (streamElapsed < STREAM_TOKENS[i].dt) {
      firstPending = i
      break
    }
  }
  const showCaret = !done && streamElapsed >= 0
  const caret = (
    <span className="tstream-cur-wrap" aria-hidden>
      <i className="tstream-cur" />
    </span>
  )

  return (
    <div className="ta-text enter">
      {STREAM_TOKENS.map((tok, i) => {
        const age = streamElapsed - tok.dt
        const visible = done || age >= 0
        const k = done ? 1 : Math.min(1, Math.max(0, age) / fadeMs)
        const opacity = visible ? from + (1 - from) * (1 - (1 - k) * (1 - k)) : 0
        const inner = tok.code ? <span className="tx-code">{tok.text}</span> : tok.text
        return (
          <span key={i} className="tok-slot">
            {showCaret && i === firstPending && caret}
            <span className="tok" style={{ opacity: opacity >= 0.999 ? undefined : +opacity.toFixed(3) }}>
              {inner}
            </span>
          </span>
        )
      })}
      {showCaret && firstPending === STREAM_TOKENS.length && caret}
    </div>
  )
}

function Session({
  step,
  fixAge,
  streamElapsed,
  brailleFrame,
  mode,
  script,
  viewRef,
  contentRef,
  children,
}: {
  viewRef: RefObject<HTMLDivElement | null>
  contentRef: RefObject<HTMLDivElement | null>
  step: number
  fixAge: number
  streamElapsed: number
  brailleFrame: number
  mode: Mode
  script: Script
  children: ReactNode
}) {
  const m = motion()
  const summary = summaryText(mode, true)
  const hasStream = streamElapsed >= 0
  const done = step >= 4

  // The scroll offset is driven by useScrollFollow (sticky bottom), never by React.
  return (
    <div className="tui-session">
      <div ref={viewRef} className="ts-view">
        <div ref={contentRef} className="ts-content">
          <div className="tu-msg enter">
            <span className="tu-bar" />
            <div className="tu-inner">
              <div className="tu-images">
                <ContactSheet mode={mode} script={script} width={300} height={8 * 15} develop />
              </div>
              <div className="tu-text">
                <div className="tu-body">{`[Image 1] ${summary.join("\n")}`}</div>
                <div className="tu-files">
                  <span className="tu-chip"> file </span>
                  <span className="tu-name"> contact-sheet.png </span>
                </div>
              </div>
            </div>
          </div>
          {step >= 1 && (
            <div className="tt-inline enter">
              <span className="tt-icon">→</span>Read src/components/HeaderCard.tsx
            </div>
          )}
          {step >= 2 && (
            <div className="tt-block enter">
              <span className="tt-bar" />
              <div className="tt-inner">
                <div className="tt-head">
                  {step === 2 ? (
                    <>
                      <Braille frame={brailleFrame} />
                      Preparing edit…
                    </>
                  ) : (
                    <span className="tt-edit-done">← Edit src/components/HeaderCard.tsx</span>
                  )}
                </div>
                {step >= 3 && (
                  <div className="reveal-rows">
                    <Diff fixAge={fixAge} tint={m.tint} />
                  </div>
                )}
              </div>
            </div>
          )}
          {(hasStream || done) && <StreamedResponse streamElapsed={streamElapsed} done={done} from={m.streamFrom} fadeMs={m.streamMs} />}
          {done && (
            <div className="ta-foot enter">
              <span className="tx-build">Build</span>
              <span className="tx-mute"> · Claude Opus 5.5 · 4.1s</span>
            </div>
          )}
        </div>
      </div>
      <div className="ts-composer">{children}</div>
    </div>
  )
}

function Home({ children }: { children: ReactNode }) {
  return (
    <div className="tui-home">
      <div className="th-grow" />
      <div className="th-gap3" />
      <Logo />
      <div className="th-gap1" />
      <div className="th-prompt">{children}</div>
      <div className="th-grow" />
      <div className="th-footer">
        <span>
          <span className="tx-ok">⊙</span> 2 MCP <span className="tx-mute">/mcps</span>
        </span>
        <span className="tx-mute">2.0.0</span>
      </div>
    </div>
  )
}

const TerminalInner = memo(function TerminalInner({
  active,
  pasteStage,
  submitted,
  step,
  fixAge,
  streamElapsed,
  spinFrame,
  brailleFrame,
  mode,
  script,
  leave,
  viewRef,
  contentRef,
}: {
  leave: number
  viewRef: RefObject<HTMLDivElement | null>
  contentRef: RefObject<HTMLDivElement | null>
  active: boolean
  pasteStage: number
  submitted: boolean
  step: number
  fixAge: number
  streamElapsed: number
  spinFrame: number
  brailleFrame: number
  mode: Mode
  script: Script
}) {
  const done = step >= 4
  const running = submitted && !done
  const title = step >= 1 ? "OC | Fix header badge and button row" : "OpenCode"
  const prompt = (
    <Prompt
      mode={mode}
      pasteStage={pasteStage}
      submitted={submitted}
      done={done}
      focused={active && !running}
      running={running}
      spinFrame={spinFrame}
    />
  )
  return (
    <div className={`term${active ? "" : " inactive"}`} style={{ left: AGENT.x, top: AGENT.y, width: AGENT.w, height: AGENT.h }}>
      <div className="term-bar">
        <TrafficLights inactive={!active} />
        <span className="term-title">{title}</span>
      </div>
      <div className="tui">
        {submitted && leave >= 0 && (
          <div className="tui-leaving" style={{ opacity: leave }} aria-hidden>
            <Home>
              <Prompt still mode={mode} pasteStage={4} submitted={false} done={false} focused={false} running={false} spinFrame={0} />
            </Home>
          </div>
        )}
        {submitted ? (
          <Session
            viewRef={viewRef}
            contentRef={contentRef}
            step={step}
            fixAge={fixAge}
            streamElapsed={streamElapsed}
            brailleFrame={brailleFrame}
            mode={mode}
            script={script}
          >
            {prompt}
          </Session>
        ) : (
          <Home>{prompt}</Home>
        )}
      </div>
    </div>
  )
})

// Sticky-bottom scrolling on a critically damped spring stepped by the demo clock: the content
// starts at the top and glides up as it overflows, keeping its velocity when a new block retargets
// it mid-glide. A seek (a backward or large step) lands at once.
function useScrollFollow(t: number, viewRef: RefObject<HTMLDivElement | null>, contentRef: RefObject<HTMLDivElement | null>, ms: number) {
  const state = useRef({ x: 0, v: 0, t: 0, el: null as HTMLDivElement | null })
  useLayoutEffect(() => {
    const view = viewRef.current
    const content = contentRef.current
    const s = state.current
    if (!view || !content) {
      s.el = null
      return
    }
    const target = Math.min(0, view.clientHeight - content.offsetHeight)
    const dt = (t - s.t) / 1000
    if (s.el !== content || dt < 0 || dt > 0.25 || ms <= 0) {
      s.x = target
      s.v = 0
    } else if (dt > 0) {
      const w = (2 * Math.PI) / (ms / 1000)
      const x0 = s.x - target
      const decay = Math.exp(-w * dt)
      const a = s.v + w * x0
      s.x = target + (x0 + a * dt) * decay
      s.v = (s.v - w * a * dt) * decay
      if (Math.abs(s.x - target) < 0.01 && Math.abs(s.v) < 0.5) {
        s.x = target
        s.v = 0
      }
    }
    s.t = t
    s.el = content
    content.style.transform = `translate3d(0, ${s.x.toFixed(2)}px, 0)`
  })
}

export function Terminal({ t, mode, script }: { t: number; mode: Mode; script: Script }) {
  const active = terminalActive(t)
  const submitted = t >= T.submit
  const step = t >= T.done ? 4 : t >= T.diff ? 3 : t >= T.edit ? 2 : t >= T.read ? 1 : 0
  // Frame-exact ages only while something is moving, so the memoized terminal skips idle frames.
  const fixAge = t >= T.fix[0] && t < T.fix[0] + 1600 ? Math.round(t - T.fix[0]) : t >= T.fix[0] + 1600 ? 1600 : -1
  const streamElapsed = t >= T.done ? 1200 : t >= STREAM_START ? Math.round(t - STREAM_START) : -1
  const running = submitted && step < 4
  const pasteStage =
    t >= T.submit || t < T.paste
      ? 0
      : t < T.paste + 90
        ? 1
        : t < T.paste + 160
          ? 2
          : t < T.paste + 230
            ? 3
            : 4
  const spinFrame = running ? Math.floor(t / 40) % 54 : 0
  const brailleFrame = step === 2 ? Math.floor(t / 80) % BRAILLE.length : 0
  // Submitting dissolves the home screen into the session over 220 ms rather than cutting.
  const leave = t >= T.submit && t < T.submit + 220 ? +(1 - smoother((t - T.submit) / 220)).toFixed(3) : -1
  const viewRef = useRef<HTMLDivElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  useScrollFollow(t, viewRef, contentRef, motion().scrollMs)
  return (
    <TerminalInner
      leave={leave}
      viewRef={viewRef}
      contentRef={contentRef}
      active={active}
      pasteStage={pasteStage}
      submitted={submitted}
      step={step}
      fixAge={fixAge}
      streamElapsed={streamElapsed}
      spinFrame={spinFrame}
      brailleFrame={brailleFrame}
      mode={mode}
      script={script}
    />
  )
}
