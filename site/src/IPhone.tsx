import type { ReactNode } from "react"

function Screen({ children }: { children?: ReactNode }) {
  return (
    <div className="ip-bezel">
      <div className="ip-screen">
        <div className="ip-status">
          <b>9:41</b>
          <span className="ip-island" />
          <span className="ip-batt" />
        </div>
        <div className="ip-nav">Deploys</div>
        <div className="ip-card">
          <div className="ip-head">
            <i className="ip-title" />
            <span className="ip-live">
              <i />
              Live
            </span>
          </div>
          {[0.72, 0.58, 0.66, 0.5].map((w, i) => (
            <div key={i} className="ip-row">
              <i style={{ width: `${w * 100}%` }} />
              <em className={i === 1 ? "fail" : ""} />
            </div>
          ))}
        </div>
        <div className="ip-rows">
          {[0.8, 0.62, 0.7].map((w, i) => (
            <i key={i} style={{ width: `${w * 100}%` }} />
          ))}
        </div>
        <div className="ip-bar">
          <span>Checkout</span>
        </div>
        {children}
      </div>
    </div>
  )
}

function MirrorVisual() {
  return (
    <div className="ip-visual" aria-hidden>
      <div className="ip-stack">
        <Screen>
          <svg className="ip-ink" viewBox="0 0 160 330" aria-hidden>
            <path d="M84 70c-18-12-62-10-66 6-4 17 46 22 76 18 26-3 40-14 28-24-11-9-39-10-58-6" />
            <path d="M144 236c8 30 2 52-20 66M124 302l11-2M124 302l3-11" />
          </svg>
          <span className="ip-pin" style={{ left: "77%", top: "16%" }}>
            1
          </span>
          <span className="ip-pin" style={{ left: "88%", top: "70%" }}>
            2
          </span>
        </Screen>
        <span className="ip-rec">
          <i />
          REC
        </span>
      </div>
    </div>
  )
}

function ScrawlKitVisual() {
  return (
    <div className="ip-visual" aria-hidden>
      <div className="ip-stack">
        <Screen>
          <svg className="ip-touch" viewBox="0 0 160 330" aria-hidden>
            <defs>
              <linearGradient id="ip-trail" x1="0" y1="1" x2="0" y2="0">
                <stop offset="0" stopColor="#0a84ff" stopOpacity="0" />
                <stop offset="1" stopColor="#0a84ff" stopOpacity="0.9" />
              </linearGradient>
            </defs>
            <circle cx="104" cy="122" r="8" className="ip-tap" />
            <circle cx="104" cy="122" r="16" className="ip-ring" />
            <circle cx="104" cy="122" r="25" className="ip-ring faint" />
            <path d="M54 262C58 226 60 200 66 168" stroke="url(#ip-trail)" strokeWidth="9" strokeLinecap="round" fill="none" />
            <circle cx="66" cy="168" r="7" className="ip-tap" />
            <circle cx="118" cy="300" r="7" className="ip-tap" />
            <circle cx="118" cy="300" r="15" className="ip-ring long" />
          </svg>
        </Screen>
      </div>
    </div>
  )
}

export function IPhoneSection() {
  return (
    <section id="iphone" className="iphone-section">
      <h2>iPhone</h2>
      <div className="feature-cards">
        <article className="feature-card">
          <MirrorVisual />
          <div className="feature-copy">
            <h3>Plug it into your Mac</h3>
            <p>Your iPhone’s screen appears in a window on the Mac, and you talk and draw over it as it records.</p>
          </div>
        </article>
        <article className="feature-card">
          <ScrawlKitVisual />
          <div className="feature-copy">
            <h3>Record inside your own app</h3>
            <p>Add ScrawlKit to your iOS app to record on the phone itself, with every tap and swipe drawn on the frames.</p>
          </div>
        </article>
      </div>
    </section>
  )
}
