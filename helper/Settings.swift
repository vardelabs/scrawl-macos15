import AVFoundation
import AppKit
import Carbon.HIToolbox
import Combine
import SwiftUI

// `scrawl-helper settings [--pane P] [--icon PNG] [--render DIR]`: the Settings window.
// Scrawl decides everything (`src/settings.rs`) and sends what to show as JSON lines on stdin:
//   {"type":"state","version":"0.3.2","login":{"on":false,"available":true,"needsApproval":false},
//    "autoSend":true,"shortcuts":[{"kind":"record","title":"Record","label":"⌥⇧S","key":"S",
//    "shift":true,"control":false,"option":true,"command":false,"isDefault":true,
//    "recording":false,"error":null},…],"microphone":{"selected":"default","value":"System default",
//    "options":[{"id":"default","title":"System default (…)","enabled":true},{"id":"-",…},…],
//    "note":null,"meter":"BuiltInMicrophoneDevice"},"transcriber":{…},"videoExport":false,
//    "retention":{"preset":"week","options":[{"id":"week","label":"…"}],"note":"…"},
//    "capturesSize":"1.2 GB","permissions":[{"permission":"microphone","status":"granted",…}],
//    "updates":true}
//   {"type":"show","pane":"permissions"}  {"type":"request"|"open-settings","permission":"…"}
//   {"type":"show","pane":"dictation","focus":{"kind":"word","entry":"Atlas"}} (or {"kind":"layouts"})
//   {"type":"close"}
// Clicks and key presses go back on stdout, e.g. {"action":"set","key":"autoSend","value":false},
// {"action":"record-shortcut","kind":"record"}, {"action":"shortcut","kind":"record",
// "keyCode":15,"modifiers":6144,"key":"r","reserved":false}, {"action":"cancel-shortcut"},
// {"action":"reset-shortcut","kind":"paste"}, {"action":"grant","permission":"camera"},
// {"action":"reopen"}, {"action":"show-captures"}, {"action":"check-for-updates"},
// {"action":"tour"}, {"action":"install-cli"}, {"action":"uninstall-cli"}, {"action":"close"},
// {"action":"edit-word","was":"Atlas","word":"Atlas Prime"},
// {"action":"edit-replacement","was":"atlis","heard":"atless","write":"Atlas"}.

// MARK: State

struct SettingsLogin: Decodable, Equatable {
  var on = false
  var available = true
  var needsApproval = false
  var error: String?
}

struct SettingsShortcut: Decodable, Equatable, Identifiable {
  var kind = "record"
  var title = "Record"
  var label = "⌥⇧S"
  var key = "S"
  var shift = true
  var control = false
  var option = true
  var command = false
  var isDefault = true
  var recording = false
  var error: String?

  var id: String { kind }
}

struct SettingsModelOption: Decodable, Equatable {
  var id: String
  var title: String
  var chosen: Bool
}

struct SettingsMenuOption: Decodable, Equatable {
  var id: String
  var title: String
  var enabled = true
}

struct SettingsMicrophone: Decodable, Equatable {
  /// "default" or a device's CoreAudio UID.
  var selected = "default"
  var value = "System default"
  /// An `id` of "-" is a separator.
  var options: [SettingsMenuOption] = [SettingsMenuOption(id: "default", title: "System default")]
  var note: String?
  /// The device to show the level of; nil until Microphone is allowed.
  var meter: String?
}

struct SettingsTranscriber: Decodable, Equatable {
  var engine = "apple"
  var hexTitle = "Hex (not running)"
  var hexEnabled = false
  var models: [SettingsModelOption] = []
  /// The program's file name while `transcribeCommand` is usable.
  var command: String?
  /// The Transcription menu, in order; an engine is enabled once it can run.
  var engines: [SettingsEngine] = [
    SettingsEngine(id: "parakeet", title: "Parakeet (not set up)", enabled: false),
    SettingsEngine(id: "whisper", title: "Whisper large-v3 turbo (not set up)", enabled: false),
    SettingsEngine(id: "apple", title: "Apple Speech", enabled: true),
    SettingsEngine(id: "hex", title: "Hex (not running)", enabled: false),
    SettingsEngine(id: "command", title: "Command (not set up)", enabled: false),
  ]
  var speech: SpeechStatus?
}

struct SettingsEngine: Decodable, Equatable {
  var id: String
  var title: String
  var enabled: Bool
}

struct SettingsOption: Decodable, Equatable {
  var id: String
  var label: String
}

struct SettingsRetention: Decodable, Equatable {
  var preset: String? = "week"
  var options: [SettingsOption] = [
    SettingsOption(id: "week", label: "Video for 7 Days"),
    SettingsOption(id: "month", label: "Video for 30 Days"),
    SettingsOption(id: "forever", label: "Forever"),
  ]
  var note = "Video and audio go to the Trash after 7 days, the rest of each capture after 30 days."
}

struct SettingsPermission: Decodable, Equatable, Identifiable {
  var permission: String
  var status = "not-determined"
  var required = true
  var requested = false
  var relaunch = false

  var id: String { permission }
  var granted: Bool { status == "granted" }
}

struct SettingsCommandLine: Decodable, Equatable {
  var path: String?
  /// "install", "uninstall", or none.
  var button: String? = "install"
  var enabled = true
  var caption = "Links scrawl into /opt/homebrew/bin, so agents and your terminal can run it."
  var error = false
}

struct SettingsReplacement: Decodable, Equatable, Hashable {
  var heard: String
  var write: String
}

struct SettingsDictation: Decodable, Equatable {
  var available = true
  var on = false
  var shortcut = "⌃⌥"
  var shortcuts = ["⌃⌥", "⌃⌘", "⌥⌘", "⌃⇧"]
  var restoreClipboard = true
  var pill = true
  var codeFormatting = true
  var formatting = true
  var formattingAvailable = false
  var learn = true
  var notices = true
  var history = true
  var canClear = false
  var learnedLayouts = 0
  var removeFillers = true
  var words: [String] = []
  var learned: [String] = []
  var replacements: [SettingsReplacement] = []
  var error: String?
  var saveError: String?
}

struct SettingsState: Decodable, Equatable {
  var version = "0.3.2"
  var login = SettingsLogin()
  var autoSend = true
  var shortcuts: [SettingsShortcut] = [
    SettingsShortcut(),
    SettingsShortcut(kind: "paste", title: "Paste last capture", label: "⌥⇧V", key: "V"),
  ]
  var microphone = SettingsMicrophone()
  var transcriber = SettingsTranscriber()
  var lowerPlayback = true
  var playbackLevel = 0
  var videoExport = false
  var tapChooses = false
  var cropWindowCaptures = true
  var dictation = SettingsDictation()
  var retention = SettingsRetention()
  var capturesSize: String?
  var permissions: [SettingsPermission] = ["screen-recording", "microphone", "accessibility", "camera"]
    .map { SettingsPermission(permission: $0, required: ["screen-recording", "microphone", "accessibility"].contains($0)) }
  var updates = true
  var commandLine = SettingsCommandLine()
  var skillCommand = "bunx skills add kitlangton/scrawl"

  var pasteLabel: String { shortcuts.first { $0.kind == "paste" }?.label ?? "⌥⇧V" }
  var recording: String? { shortcuts.first(where: \.recording)?.kind }
}

enum SettingsPane: String, CaseIterable {
  case general, recording, dictation, storage, permissions, agents, about

  var title: String {
    switch self {
    case .general: "General"
    case .recording: "Recording"
    case .dictation: "Dictation"
    case .storage: "Storage"
    case .permissions: "Permissions"
    case .agents: "Agents"
    case .about: "About"
    }
  }

  var symbol: String {
    switch self {
    case .general: "gearshape.fill"
    case .recording: "record.circle"
    case .dictation: "waveform"
    case .storage: "internaldrive.fill"
    case .permissions: "hand.raised.fill"
    case .agents: "terminal.fill"
    case .about: "info"
    }
  }

  /// The icon tile's gradient, top to bottom.
  var tint: (Color, Color) {
    switch self {
    case .general, .about: (Color(white: 0.62), Color(white: 0.45))
    case .recording: (Color(red: 1, green: 0.42, blue: 0.38), Color(red: 0.92, green: 0.2, blue: 0.17))
    case .dictation: (Color(red: 0.72, green: 0.5, blue: 1), Color(red: 0.52, green: 0.3, blue: 0.93))
    case .storage: (Color(red: 0.36, green: 0.66, blue: 1), Color(red: 0.16, green: 0.45, blue: 0.95))
    case .permissions: (Color(red: 0.36, green: 0.82, blue: 0.56), Color(red: 0.16, green: 0.66, blue: 0.4))
    case .agents: (Color(white: 0.36), Color(white: 0.18))
    }
  }
}

/// A dictionary entry open for editing in place, with what has been typed so far.
struct DictionaryEditing: Equatable {
  enum Kind { case word, replacement }
  var kind: Kind
  /// The entry as saved: the word, or the replacement's heard words.
  var was: String
  var text: String
  /// A replacement's written words.
  var write = ""
  /// Why the typed entry can't be saved.
  var refusal: String?

  static func word(_ word: String) -> Self { Self(kind: .word, was: word, text: word) }
  static func replacement(_ entry: SettingsReplacement) -> Self {
    Self(kind: .replacement, was: entry.heard, text: entry.heard, write: entry.write)
  }
}

final class SettingsModel: ObservableObject {
  @Published var state = SettingsState()
  @Published var pane: SettingsPane
  @Published var editing: DictionaryEditing?
  /// The row a pill notice opened the pane at ("word:Atlas", "layouts"), lit for a moment.
  @Published var highlight: String?
  /// Counts focus requests, so a second click on the same notice scrolls again.
  @Published var focusRequests = 0
  /// Rendering: how far down the pane is scrolled, since ImageRenderer can't draw a scroll view.
  var renderScroll: CGFloat = 0
  /// Modifiers held while a recorder listens, so their keycaps light as they go down.
  @Published var liveModifiers: NSEvent.ModifierFlags = []
  let icon: NSImage?
  /// Rendering to PNGs: draw stand-in traffic lights where the window's would be.
  var rendering = false
  let meter = MicMeter()
  var send: ([String: Any]) -> Void = { _ in }

  init(pane: SettingsPane, icon: NSImage?) {
    self.pane = pane
    self.icon = icon
  }

  func act(_ action: String, _ fields: [String: Any] = [:]) {
    var message = fields
    message["action"] = action
    send(message)
  }

  func set(_ key: String, _ value: Any) {
    act("set", ["key": key, "value": value])
  }

  /// A pill notice's click: the Dictation pane at a learned word, open for editing, or at the
  /// learned-layouts row.
  func focus(_ focus: [String: Any]) {
    let row: String
    switch (focus["kind"] as? String, focus["entry"] as? String) {
    case ("word", let word?):
      row = DictionaryRows.word(word)
      if state.dictation.words.contains(word) { startEditing(.word(word)) }
    case ("layouts", _):
      row = DictionaryRows.layouts
    default:
      return
    }
    highlight = row
    focusRequests += 1
    DispatchQueue.main.asyncAfter(deadline: .now() + 1.8) { [weak self] in
      if self?.highlight == row { self?.highlight = nil }
    }
  }

  /// Opens an entry for editing, saving the one open now first; one that can't be saved stays.
  func startEditing(_ edit: DictionaryEditing) {
    saveEditing()
    if editing == nil { editing = edit }
  }

  /// Saves the entry being edited (only `was`, when given), or says why it can't be; an
  /// unchanged one just closes.
  func saveEditing(_ was: String? = nil) {
    guard var edit = editing, was == nil || was == edit.was else { return }
    let d = state.dictation
    let text = edit.text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    let write = edit.write.trimmingCharacters(in: .whitespaces)
    edit.refusal = DictionaryRows.refusal(edit, text: text, write: write, in: d)
    if edit.refusal != nil {
      editing = edit
      return
    }
    editing = nil
    switch edit.kind {
    case .word where text != edit.was:
      act("edit-word", ["was": edit.was, "word": text])
    case .replacement where text != edit.was || write != d.replacements.first(where: { $0.heard == edit.was })?.write:
      act("edit-replacement", ["was": edit.was, "heard": text, "write": write])
    default:
      break
    }
  }
}

/// Row ids for scrolling to a dictionary entry, and the checks Scrawl makes again when it saves
/// one (`Dictionary::edit_word`, `edit_replacement`), here so a refusal shows as the user types.
enum DictionaryRows {
  static let layouts = "layouts"
  static func word(_ word: String) -> String { "word:\(word)" }
  static func replacement(_ heard: String) -> String { "replacement:\(heard)" }

  /// Words as the dictionary compares them: runs of letters, digits and apostrophes, lower-cased.
  static func words(_ text: String) -> [String] {
    text.lowercased().split { !($0.isLetter || $0.isNumber || $0 == "'") }.map(String.init)
  }

  static func refusal(_ edit: DictionaryEditing, text: String, write: String, in d: SettingsDictation) -> String? {
    let words = words(text)
    let taken: Bool
    switch edit.kind {
    case .word:
      if words.isEmpty { return "An entry needs a word." }
      taken = d.words.contains { $0 != edit.was && Self.words($0) == words }
    case .replacement:
      if words.isEmpty || write.isEmpty { return "An entry needs a word." }
      taken = d.replacements.contains { $0.heard != edit.was && Self.words($0.heard) == words }
    }
    return taken ? "That's already in your dictionary." : nil
  }
}

// MARK: Look

struct SettingsTheme {
  let dark: Bool

  init(_ scheme: ColorScheme) { dark = scheme == .dark }

  private func ink(_ alpha: Double) -> Color { dark ? Color.white.opacity(alpha) : Color.black.opacity(alpha) }

  var window: Color { dark ? Color(white: 0.118) : Color(white: 0.955) }
  var sidebar: Color { dark ? Color(white: 0.15) : Color(white: 0.915) }
  var group: Color { dark ? Color.white.opacity(0.045) : Color.white }
  var groupLine: Color { ink(dark ? 0.075 : 0.08) }
  var separator: Color { ink(dark ? 0.07 : 0.075) }
  var text: Color { dark ? Color(white: 0.95) : Color(white: 0.1) }
  var secondary: Color { ink(dark ? 0.55 : 0.5) }
  var tertiary: Color { ink(dark ? 0.3 : 0.28) }
  var selection: Color { ink(dark ? 0.08 : 0.07) }
  var control: Color { ink(dark ? 0.08 : 0.06) }
  var controlLine: Color { ink(dark ? 0.1 : 0.1) }
  var track: Color { ink(dark ? 0.12 : 0.13) }
  var key: Color { dark ? Color.black.opacity(0.28) : Color.black.opacity(0.035) }
  var keyLine: Color { ink(dark ? 0.08 : 0.09) }
  var accent: Color { Color.accentColor }
  var positive: Color { Color(red: 0.2, green: 0.74, blue: 0.42) }
  var negative: Color { Color(red: 1, green: 0.27, blue: 0.23) }
}

private struct ThemeKey: EnvironmentKey {
  static let defaultValue = SettingsTheme(.dark)
}

private struct RenderingKey: EnvironmentKey {
  static let defaultValue = false
}

extension EnvironmentValues {
  var settingsTheme: SettingsTheme {
    get { self[ThemeKey.self] }
    set { self[ThemeKey.self] = newValue }
  }

  /// Drawn by `ImageRenderer`, which can't draw AppKit-backed controls such as text fields.
  var settingsRendering: Bool {
    get { self[RenderingKey.self] }
    set { self[RenderingKey.self] = newValue }
  }
}

enum SettingsMotion {
  static let reduce = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
  /// Panes cross-fade in place: no travel, so reduced motion keeps the same fade.
  static let paneIn: Animation = .easeOut(duration: 0.16)
  static let paneOut: Animation = .easeIn(duration: 0.12)
  /// Switch knobs, keycaps and captions answering Scrawl (bounce 0).
  static let state: Animation = reduce ? .easeOut(duration: 0.15) : .spring(duration: 0.25, bounce: 0)
}

/// Sizes: 16 titles, 14 rows, 12 captions, 11 keycap names, 20 keycap letters.
enum SettingsMetrics {
  static let width: CGFloat = 780
  static let height: CGFloat = 600
  static let minWidth: CGFloat = width
  static let minHeight: CGFloat = height
  static let sidebar: CGFloat = 210
  static let titlebar: CGFloat = 52
  static let inset: CGFloat = 20
  static let row: CGFloat = 42
  static let radius: CGFloat = 12
}

// MARK: Controls

struct IconTile: View {
  let symbol: String
  let tint: (Color, Color)
  var size: CGFloat = 24

  var body: some View {
    RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
      .fill(LinearGradient(colors: [tint.0, tint.1], startPoint: .top, endPoint: .bottom))
      .overlay(
        RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
          .strokeBorder(Color.white.opacity(0.14), lineWidth: 0.5)
      )
      .overlay(
        Image(systemName: symbol)
          .font(.system(size: size * 0.5, weight: .semibold))
          .foregroundStyle(.white)
      )
      .frame(width: size, height: size)
  }
}

struct SettingsSwitch: View {
  @Environment(\.settingsTheme) private var theme
  let on: Bool
  var enabled = true
  let action: (Bool) -> Void

  var body: some View {
    Button { action(!on) } label: {
      Capsule()
        .fill(on ? theme.accent : theme.track)
        .overlay(alignment: on ? .trailing : .leading) {
          Circle()
            .fill(Color.white)
            .shadow(color: .black.opacity(0.22), radius: 1, y: 0.5)
            .padding(2)
        }
        .frame(width: 38, height: 22)
        .contentShape(Capsule())
    }
    .buttonStyle(.plain)
    .opacity(enabled ? 1 : 0.4)
    .disabled(!enabled)
    .animation(SettingsMotion.reduce ? nil : SettingsMotion.state, value: on)
  }
}

final class MenuPick: NSObject {
  let handler: (String) -> Void
  init(_ handler: @escaping (String) -> Void) { self.handler = handler }
  @objc func pick(_ item: NSMenuItem) {
    if let id = item.representedObject as? String { handler(id) }
  }
}

/// A value with up/down chevrons that opens a menu of choices where it was clicked, positioned
/// so the current choice sits under the pointer (like a pop-up button).
struct SettingsPopup: View {
  @Environment(\.settingsTheme) private var theme
  let value: String
  let options: [(id: String, title: String, enabled: Bool)]
  let selected: String?
  let pick: (String) -> Void

  var body: some View {
    Button(action: open) {
      HStack(spacing: 8) {
        Text(value)
          .font(.system(size: 14))
          .foregroundStyle(theme.text.opacity(0.85))
          .lineLimit(1)
          .truncationMode(.middle)
          .frame(maxWidth: 190, alignment: .trailing)
        Image(systemName: "chevron.up.chevron.down")
          .font(.system(size: 9, weight: .bold))
          .foregroundStyle(theme.secondary)
          .frame(width: 20, height: 20)
          .overlay(Circle().strokeBorder(theme.controlLine, lineWidth: 1))
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
  }

  private func open() {
    let menu = NSMenu()
    menu.autoenablesItems = false
    let target = MenuPick(pick)
    var current: NSMenuItem?
    for option in options {
      if option.id == "-" {
        menu.addItem(.separator())
        continue
      }
      let item = NSMenuItem(title: option.title, action: #selector(MenuPick.pick(_:)), keyEquivalent: "")
      item.target = target
      item.representedObject = option.id
      item.isEnabled = option.enabled
      item.state = option.id == selected ? .on : .off
      menu.addItem(item)
      if option.id == selected { current = item }
    }
    guard let event = NSApp.currentEvent, let view = event.window?.contentView else { return }
    let point = view.convert(event.locationInWindow, from: nil)
    withExtendedLifetime(target) { _ = menu.popUp(positioning: current, at: point, in: view) }
  }
}

struct SettingsButton: View {
  @Environment(\.settingsTheme) private var theme
  let title: String
  var enabled = true
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(.system(size: 12, weight: .medium))
        .foregroundStyle(theme.text)
        .padding(.horizontal, 11)
        .frame(height: 24)
        .background(Capsule().fill(theme.control))
        .overlay(Capsule().strokeBorder(theme.controlLine, lineWidth: 1))
        .contentShape(Capsule())
    }
    .buttonStyle(.plain)
    .opacity(enabled ? 1 : 0.45)
    .disabled(!enabled)
  }
}

/// A rounded container of rows with hairline separators between them.
struct SettingsGroup<Content: View>: View {
  @Environment(\.settingsTheme) private var theme
  @ViewBuilder let content: () -> Content

  var body: some View {
    VStack(spacing: 0) { content() }
      .background(RoundedRectangle(cornerRadius: SettingsMetrics.radius, style: .continuous).fill(theme.group))
      .overlay(RoundedRectangle(cornerRadius: SettingsMetrics.radius, style: .continuous).strokeBorder(theme.groupLine, lineWidth: 1))
  }
}

struct SettingsSeparator: View {
  @Environment(\.settingsTheme) private var theme

  var body: some View {
    Rectangle().fill(theme.separator).frame(height: 1)
  }
}

struct SettingsRow<Trailing: View>: View {
  @Environment(\.settingsTheme) private var theme
  let title: String
  var enabled = true
  @ViewBuilder let trailing: () -> Trailing

  var body: some View {
    HStack(spacing: 12) {
      Text(title)
        .font(.system(size: 14))
        .foregroundStyle(enabled ? theme.text : theme.secondary)
      Spacer(minLength: 8)
      trailing()
    }
    .padding(.horizontal, 12)
    .frame(height: SettingsMetrics.row)
  }
}

/// A group's heading, eight points in from the group's edge, with an optional trailing control.
struct SettingsHeader<Trailing: View>: View {
  @Environment(\.settingsTheme) private var theme
  let title: String
  @ViewBuilder let trailing: () -> Trailing

  var body: some View {
    HStack {
      Text(title)
        .font(.system(size: 13, weight: .semibold))
        .foregroundStyle(theme.secondary)
      Spacer()
      trailing()
    }
    .padding(.horizontal, 8)
    .frame(height: 20)
    .padding(.top, 20)
    .padding(.bottom, 6)
  }
}

extension SettingsHeader where Trailing == EmptyView {
  init(_ title: String) {
    self.title = title
    trailing = { EmptyView() }
  }
}

/// A saved word, or a replacement as "heard → write", with a remove button. A double click edits
/// it in place.
struct DictionaryEntryRow: View {
  @Environment(\.settingsTheme) private var theme
  let title: String
  var detail: String?
  var lit = false
  let edit: () -> Void
  let remove: () -> Void

  var body: some View {
    HStack(spacing: 8) {
      HStack(spacing: 8) {
        Text(title).font(.system(size: 14)).foregroundStyle(theme.text).lineLimit(1)
        if let detail {
          Image(systemName: "arrow.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(theme.tertiary)
          Text(detail).font(.system(size: 14)).foregroundStyle(theme.text).lineLimit(1)
        }
        Spacer(minLength: 8)
      }
      .contentShape(Rectangle())
      .onTapGesture(count: 2, perform: edit)
      .help("Double-click to edit")
      Button(action: remove) {
        Image(systemName: "minus.circle.fill")
          .font(.system(size: 15))
          .foregroundStyle(theme.secondary)
          .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .help("Remove")
    }
    .padding(.horizontal, 12)
    .frame(height: SettingsMetrics.row)
    .background(DictionaryHighlight(lit: lit))
  }
}

/// The wash behind the row a pill notice opened the pane at.
struct DictionaryHighlight: View {
  @Environment(\.settingsTheme) private var theme
  let lit: Bool

  var body: some View {
    Rectangle().fill(theme.accent.opacity(lit ? 0.2 : 0)).animation(.easeOut(duration: 0.6), value: lit)
  }
}

/// A word or replacement open for editing: Return or a click elsewhere saves, Esc cancels. An
/// entry that can't be saved stays open with the reason under it.
struct DictionaryEditRow: View {
  private enum Field { case text, write }
  @Environment(\.settingsTheme) private var theme
  @Environment(\.settingsRendering) private var rendering
  @Binding var edit: DictionaryEditing
  var lit = false
  let save: () -> Void
  let cancel: () -> Void
  @FocusState private var focused: Field?
  @State private var closed = false

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      HStack(spacing: 8) {
        field($edit.text, .text, placeholder: edit.kind == .word ? "Word or phrase" : "Heard")
        if edit.kind == .replacement {
          Image(systemName: "arrow.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(theme.tertiary)
          field($edit.write, .write, placeholder: "Write")
        }
      }
      if let refusal = edit.refusal {
        Text(refusal).font(.system(size: 12)).foregroundStyle(theme.negative).padding(.leading, 7)
      }
    }
    .padding(.horizontal, 5)
    .padding(.top, 6)
    .padding(.bottom, edit.refusal == nil ? 6 : 10)
    .frame(minHeight: SettingsMetrics.row)
    .background(DictionaryHighlight(lit: lit))
    .onAppear { DispatchQueue.main.async { focused = .text } }
    .onChange(of: focused) { _, now in
      if now == nil, !closed { finish(save) }
    }
  }

  @ViewBuilder private func field(_ text: Binding<String>, _ which: Field, placeholder: String) -> some View {
    let look = RoundedRectangle(cornerRadius: 6, style: .continuous)
    Group {
      if rendering {
        HStack(spacing: 1) {
          Text(text.wrappedValue).font(.system(size: 14)).foregroundStyle(theme.text)
          if which == .text { Rectangle().fill(theme.accent).frame(width: 1.5, height: 16) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
      } else {
        TextField(placeholder, text: text)
          .textFieldStyle(.plain)
          .font(.system(size: 14))
          .foregroundStyle(theme.text)
          .focused($focused, equals: which)
          .onSubmit { finish(save) }
          .onExitCommand { finish(cancel) }
          .onChange(of: text.wrappedValue) { edit.refusal = nil }
      }
    }
    .padding(.horizontal, 7)
    .frame(height: 28)
    .background(look.fill(theme.control))
    .overlay(look.strokeBorder(edit.refusal == nil ? theme.accent : theme.negative, lineWidth: 1.5))
  }

  /// Saving may leave the row open with a refusal, so only a close ends it.
  private func finish(_ action: () -> Void) {
    action()
    closed = edit.refusal == nil
  }
}

struct DictionaryField: View {
  @Environment(\.settingsTheme) private var theme
  @Environment(\.settingsRendering) private var rendering
  let placeholder: String
  @Binding var text: String
  let submit: () -> Void

  var body: some View {
    if rendering {
      Text(placeholder).font(.system(size: 14)).foregroundStyle(theme.tertiary)
        .frame(maxWidth: .infinity, alignment: .leading)
    } else {
      TextField(placeholder, text: $text)
        .textFieldStyle(.plain)
        .font(.system(size: 14))
        .foregroundStyle(theme.text)
        .onSubmit(submit)
    }
  }
}

/// The last row of the Words group: type a word or phrase and press Return or Add.
struct DictionaryAddRow: View {
  let placeholder: String
  var enabled = true
  let add: (String) -> Void
  @State private var text = ""

  var body: some View {
    HStack(spacing: 8) {
      DictionaryField(placeholder: placeholder, text: $text, submit: commit)
      SettingsButton(title: "Add", enabled: enabled && !text.trimmingCharacters(in: .whitespaces).isEmpty, action: commit)
    }
    .padding(.horizontal, 12)
    .frame(height: SettingsMetrics.row)
    .disabled(!enabled)
  }

  private func commit() {
    let word = text.trimmingCharacters(in: .whitespaces)
    guard enabled, !word.isEmpty else { return }
    add(word)
    text = ""
  }
}

/// The last row of the Replacements group: what is heard, what to write instead.
struct ReplacementAddRow: View {
  @Environment(\.settingsTheme) private var theme
  var enabled = true
  let add: (String, String) -> Void
  @State private var heard = ""
  @State private var write = ""

  var body: some View {
    HStack(spacing: 8) {
      DictionaryField(placeholder: "Heard", text: $heard, submit: commit)
      Image(systemName: "arrow.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(theme.tertiary)
      DictionaryField(placeholder: "Write", text: $write, submit: commit)
      SettingsButton(title: "Add", enabled: enabled && ready, action: commit)
    }
    .padding(.horizontal, 12)
    .frame(height: SettingsMetrics.row)
    .disabled(!enabled)
  }

  private var ready: Bool {
    !heard.trimmingCharacters(in: .whitespaces).isEmpty && !write.trimmingCharacters(in: .whitespaces).isEmpty
  }

  private func commit() {
    guard enabled, ready else { return }
    add(heard.trimmingCharacters(in: .whitespaces), write.trimmingCharacters(in: .whitespaces))
    heard = ""
    write = ""
  }
}

struct SettingsCaption: View {
  @Environment(\.settingsTheme) private var theme
  let text: String
  var error = false

  var body: some View {
    Text(text)
      .font(.system(size: 12))
      .foregroundStyle(error ? theme.negative : theme.secondary)
      .lineSpacing(2)
      .fixedSize(horizontal: false, vertical: true)
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.horizontal, 8)
      .padding(.top, 7)
  }
}

// MARK: Input level

/// The level of the microphone recordings will use, only while the Recording pane is on screen
/// and only once Microphone is allowed (Scrawl sends `meter` then). Its own object, so the level
/// redraws the meter and nothing else.
final class MicMeter: NSObject, ObservableObject, AVCaptureAudioDataOutputSampleBufferDelegate {
  /// 0…1, rising at once and falling at `release` a tick, like a level meter.
  @Published var level: Float = 0
  private var device: String?
  private var session: AVCaptureSession?
  private var output: AVCaptureAudioDataOutput?
  private var timer: Timer?
  private let queue = DispatchQueue(label: "dev.kitlangton.scrawl.helper.meter")
  private static let floor: Float = -50
  private static let release: Float = 0.035

  /// Meters `id` (a CoreAudio UID), or stops with nil.
  func meter(_ id: String?) {
    guard id != device else { return }
    device = id
    timer?.invalidate()
    timer = nil
    output = nil
    if let old = session {
      queue.async { old.stopRunning() }
      session = nil
    }
    level = 0
    guard let id, AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { return }
    let session = AVCaptureSession()
    let output = AVCaptureAudioDataOutput()
    output.setSampleBufferDelegate(self, queue: queue)
    self.session = session
    self.output = output
    queue.async {
      guard let device = AVCaptureDevice(uniqueID: id), let input = try? AVCaptureDeviceInput(device: device) else { return }
      session.beginConfiguration()
      if session.canAddInput(input) { session.addInput(input) }
      if session.canAddOutput(output) { session.addOutput(output) }
      session.commitConfiguration()
      session.startRunning()
    }
    timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in self?.tick() }
  }

  private func tick() {
    let channels = output?.connections.first?.audioChannels ?? []
    let db = channels.map(\.averagePowerLevel).max() ?? -160
    let target = max(0, min(1, (db - Self.floor) / -Self.floor))
    let next = target >= level ? target : max(target, level - Self.release)
    if next != level { level = next }
  }

  /// The connection's channels carry the levels; the samples themselves aren't needed.
  func captureOutput(_: AVCaptureOutput, didOutput _: CMSampleBuffer, from _: AVCaptureConnection) {}
}

/// Sixteen segments, lit left to right with the level.
struct LevelMeter: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var meter: MicMeter
  static let segments = 16

  var body: some View {
    let lit = Int((meter.level * Float(Self.segments)).rounded())
    HStack(spacing: 3) {
      ForEach(0..<Self.segments, id: \.self) { i in
        RoundedRectangle(cornerRadius: 1.5, style: .continuous)
          .fill(i < lit ? theme.text.opacity(0.7) : theme.track)
          .frame(width: 5, height: 12)
      }
    }
    .accessibilityElement()
    .accessibilityLabel("Input level")
    .accessibilityValue("\(Int(meter.level * 100)) percent")
  }
}

// MARK: Shortcut recorder

/// One key, Klack style: a modifier shows its glyph top left and its name bottom left; the key
/// itself is a centred letter. Lit when part of the shortcut, dim otherwise.
struct SettingsKeycap: View {
  @Environment(\.settingsTheme) private var theme
  let glyph: String
  var name: String?
  var lit: Bool
  var center = false

  var body: some View {
    ZStack(alignment: .topLeading) {
      RoundedRectangle(cornerRadius: 9, style: .continuous)
        .fill(theme.key)
        .overlay(
          RoundedRectangle(cornerRadius: 9, style: .continuous)
            .strokeBorder(theme.keyLine, lineWidth: 1)
        )
      if center {
        Text(glyph)
          .font(.system(size: glyph.count > 2 ? 14 : 20, weight: .medium))
          .foregroundStyle(lit ? theme.text : theme.tertiary)
          .frame(maxWidth: .infinity, maxHeight: .infinity)
      } else {
        VStack(alignment: .leading, spacing: 0) {
          if name != nil {
            Text(glyph).font(.system(size: 15))
          }
          Spacer(minLength: 0)
          Text(name ?? glyph).font(.system(size: name == nil ? 15 : 13))
        }
        .foregroundStyle(lit ? theme.text : theme.tertiary)
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
      }
    }
    .frame(height: 52)
  }
}

struct ShortcutRecorder: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel
  let shortcut: SettingsShortcut
  var tapChooses = false
  /// A setting that belongs to this shortcut, drawn in the same box under its keys.
  var footer: AnyView? = nil

  var body: some View {
    let live = shortcut.recording ? model.liveModifiers : []
    let lit = { (on: Bool, flag: NSEvent.ModifierFlags) in shortcut.recording ? live.contains(flag) : on }
    VStack(spacing: 0) {
      SettingsHeader(title: shortcut.title) {
        Button { model.act("reset-shortcut", ["kind": shortcut.kind]) } label: {
          Image(systemName: "arrow.counterclockwise")
            .font(.system(size: 9, weight: .bold))
            .foregroundStyle(theme.secondary)
            .frame(width: 20, height: 20)
            .background(Circle().fill(theme.control))
            .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .help("Reset to \(shortcut.kind == "paste" ? "⌥⇧V" : "⌥⇧S")")
        .opacity(shortcut.isDefault || shortcut.recording ? 0 : 1)
        .disabled(shortcut.isDefault || shortcut.recording)
      }
      VStack(spacing: 0) {
        Button {
          if shortcut.recording {
            model.act("cancel-shortcut")
          } else {
            model.act("record-shortcut", ["kind": shortcut.kind])
          }
        } label: {
          HStack(spacing: 6) {
            SettingsKeycap(glyph: "⇧", lit: lit(shortcut.shift, .shift))
            SettingsKeycap(glyph: "⌃", name: "ctrl", lit: lit(shortcut.control, .control))
            SettingsKeycap(glyph: "⌥", name: "opt", lit: lit(shortcut.option, .option))
            SettingsKeycap(glyph: "⌘", name: "cmd", lit: lit(shortcut.command, .command))
            SettingsKeycap(glyph: shortcut.recording ? "…" : shortcut.key, lit: !shortcut.recording, center: true)
          }
          .padding(7)
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        if let footer {
          SettingsSeparator()
          footer
        }
      }
      .background(RoundedRectangle(cornerRadius: SettingsMetrics.radius + 2, style: .continuous).fill(theme.group))
      .overlay(
        RoundedRectangle(cornerRadius: SettingsMetrics.radius + 2, style: .continuous)
          .strokeBorder(shortcut.recording ? theme.accent : theme.groupLine, lineWidth: shortcut.recording ? 1.5 : 1)
      )
      ZStack(alignment: .leading) {
        SettingsCaption(text: caption, error: shortcut.error != nil && !shortcut.recording)
          .id(caption)
          .transition(.opacity)
      }
      .frame(height: 26, alignment: .top)
    }
    .animation(SettingsMotion.state, value: shortcut)
  }

  private var caption: String {
    if shortcut.recording { return "Press the new shortcut. Esc cancels." }
    if let error = shortcut.error { return error }
    if shortcut.kind == "paste" { return "Pastes the last capture into the app in front." }
    return tapChooses
      ? "Opens the picker to choose what to record. Press again to finish."
      : "Records the screen. Press again to finish."
  }
}

// MARK: Panes

struct SettingsView: View {
  @ObservedObject var model: SettingsModel
  @Environment(\.colorScheme) private var scheme

  var body: some View {
    let theme = SettingsTheme(scheme)
    HStack(spacing: 0) {
      sidebar
        .frame(width: SettingsMetrics.sidebar)
        .background(theme.sidebar)
      Rectangle().fill(theme.separator).frame(width: 1)
      ZStack(alignment: .topLeading) {
        content(model.pane)
          .id(model.pane)
          .transition(.asymmetric(
            insertion: .opacity.animation(SettingsMotion.paneIn),
            removal: .opacity.animation(SettingsMotion.paneOut)
          ))
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
      .background(theme.window)
      .contentShape(Rectangle())
      .onTapGesture {
        if model.state.recording != nil { model.act("cancel-shortcut") }
        // A click away from an entry being edited saves it, as Return does.
        if model.editing != nil { NSApp.keyWindow?.makeFirstResponder(nil) }
      }
    }
    .frame(minWidth: SettingsMetrics.minWidth, maxWidth: .infinity,
           minHeight: SettingsMetrics.minHeight, maxHeight: .infinity)
    .environment(\.settingsTheme, theme)
    .environment(\.settingsRendering, model.rendering)
  }

  // Sidebar

  private var sidebar: some View {
    VStack(alignment: .leading, spacing: 0) {
      if model.rendering { trafficLights } else { Spacer().frame(height: SettingsMetrics.titlebar) }
      item(.general)
      section("Settings")
      VStack(spacing: 2) {
        item(.recording)
        item(.dictation)
        item(.storage)
        item(.permissions)
        item(.agents)
      }
      section("Scrawl")
      item(.about)
      Spacer()
    }
    .padding(.horizontal, 10)
  }

  private var trafficLights: some View {
    HStack(spacing: 8) {
      Circle().fill(Color(red: 1, green: 0.37, blue: 0.34))
      Circle().fill(Color(red: 1, green: 0.74, blue: 0.18))
      Circle().fill(Color(red: 0.16, green: 0.79, blue: 0.25))
    }
    .frame(width: 52, height: 12)
    .padding(.leading, 10)
    .frame(height: SettingsMetrics.titlebar, alignment: .center)
  }

  private func section(_ title: String) -> some View {
    Text(title)
      .font(.system(size: 12, weight: .semibold))
      .foregroundStyle(SettingsTheme(scheme).secondary)
      .padding(.leading, 10)
      .padding(.top, 16)
      .padding(.bottom, 4)
  }

  private func item(_ pane: SettingsPane) -> some View {
    let theme = SettingsTheme(scheme)
    let selected = model.pane == pane
    return Button {
      if model.state.recording != nil { model.act("cancel-shortcut") }
      model.pane = pane
    } label: {
      HStack(spacing: 10) {
        IconTile(symbol: pane.symbol, tint: pane.tint)
        Text(pane.title)
          .font(.system(size: 14))
          .foregroundStyle(theme.text)
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 8)
      .frame(height: 36)
      .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(selected ? theme.selection : .clear))
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
  }

  // Content

  private func content(_ pane: SettingsPane) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 10) {
        IconTile(symbol: pane.symbol, tint: pane.tint)
        Text(pane.title)
          .font(.system(size: 16, weight: .semibold))
          .foregroundStyle(SettingsTheme(scheme).text)
      }
      .frame(height: SettingsMetrics.titlebar)
      .padding(.horizontal, SettingsMetrics.inset)
      if model.rendering {
        // ImageRenderer cannot draw the AppKit-backed scroll view; an explicit height keeps the
        // top of a pane taller than the window in view.
        paneBody(pane)
          .padding(.horizontal, SettingsMetrics.inset)
          .offset(y: -model.renderScroll)
          .frame(height: SettingsMetrics.height - SettingsMetrics.titlebar, alignment: .top).clipped()
      } else {
        // Every pane scrolls, so none is cut off at the window's minimum size. The inset is
        // inside the scroll view, so its bar sits at the window's edge, clear of the groups.
        ScrollViewReader { proxy in
          ScrollView {
            paneBody(pane)
              .padding(.horizontal, SettingsMetrics.inset)
              .padding(.bottom, 16)
          }
          .onAppear { scrollToHighlight(proxy) }
          .onChange(of: model.focusRequests) { scrollToHighlight(proxy) }
        }
      }
      Spacer(minLength: 0)
    }
  }

  /// To the row a pill notice opened the pane at, once the pane has laid out.
  private func scrollToHighlight(_ proxy: ScrollViewProxy) {
    guard let row = model.highlight else { return }
    DispatchQueue.main.async {
      withAnimation(SettingsMotion.reduce ? nil : .easeInOut(duration: 0.3)) { proxy.scrollTo(row, anchor: .center) }
    }
  }

  @ViewBuilder private func paneBody(_ pane: SettingsPane) -> some View {
    switch pane {
    case .general: general
    case .recording: recording
    case .dictation: dictation
    case .storage: storage
    case .permissions: permissions
    case .agents: agents
    case .about: about
    }
  }

  private var state: SettingsState { model.state }

  private var general: some View {
    VStack(spacing: 0) {
      SettingsGroup {
        SettingsRow(title: "Launch at login", enabled: state.login.available) {
          SettingsSwitch(on: state.login.on, enabled: state.login.available) { model.set("launchAtLogin", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Send dev-site captures to OpenCode") {
          SettingsSwitch(on: state.autoSend) { model.set("autoSend", $0) }
        }
      }
      if let error = state.login.error {
        SettingsCaption(text: "Launch at login failed: \(error)", error: true)
      } else if state.login.needsApproval {
        SettingsCaption(text: "Allow Scrawl in System Settings › General › Login Items.")
      } else if !state.login.available {
        SettingsCaption(text: "Launch at login works in the installed Scrawl.app.")
      }
      ForEach(state.shortcuts) { shortcut in
        ShortcutRecorder(
          model: model, shortcut: shortcut, tapChooses: state.tapChooses,
          footer: shortcut.kind == "record"
            ? AnyView(SettingsRow(title: "Tap opens the picker") {
                SettingsSwitch(on: state.tapChooses) { model.set("tapChooses", $0) }
              })
            : nil)
      }
    }
  }

  private var dictation: some View {
    let d = state.dictation
    let usage = d.available
      ? "Hold \(d.shortcut) while you talk. Let go to paste where you're typing.\nFor hands-free, double-tap \(d.shortcut) or click the pill. Press \(d.shortcut) again or click ✓ to paste.\nPress Esc to cancel."
      : "Dictation works on macOS."
    return VStack(spacing: 0) {
      SettingsGroup {
        SettingsRow(title: "Dictation", enabled: d.available) {
          SettingsSwitch(on: d.on, enabled: d.available) { model.set("dictation", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Shortcut", enabled: d.available) {
          SettingsPopup(value: d.shortcut, options: d.shortcuts.map { ($0, $0, true) }, selected: d.shortcut) {
            model.set("dictationShortcut", $0)
          }
        }
        SettingsSeparator()
        SettingsRow(title: "Always show the pill", enabled: d.available) {
          SettingsSwitch(on: d.pill, enabled: d.available) { model.set("dictationPill", $0) }
        }
      }
      SettingsCaption(text: usage)
      SettingsHeader("Pasting")
      SettingsGroup {
        SettingsRow(title: "Restore the clipboard after a paste") {
          SettingsSwitch(on: d.restoreClipboard) { model.set("dictationRestoreClipboard", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Remove filler words") {
          SettingsSwitch(on: d.removeFillers) { model.set("removeFillers", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Format code and filenames") {
          SettingsSwitch(on: d.codeFormatting) { model.set("dictationCodeFormatting", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Format paragraphs and lists", enabled: d.formattingAvailable) {
          SettingsSwitch(on: d.formatting && d.formattingAvailable, enabled: d.formattingAvailable) { model.set("dictationFormatting", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Learn from my edits") {
          SettingsSwitch(on: d.learn) { model.set("dictationLearn", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Show what dictation learns") {
          SettingsSwitch(on: d.notices) { model.set("dictationNotices", $0) }
        }
        SettingsSeparator()
        SettingsRow(title: "Keep recent dictations") {
          HStack(spacing: 8) {
            SettingsButton(title: "Clear", enabled: d.canClear) { model.act("clear-dictations") }
            SettingsSwitch(on: d.history) { model.set("dictationHistory", $0) }
          }
        }
        SettingsSeparator()
        SettingsRow(title: d.learnedLayouts == 1 ? "1 layout learned" : "\(d.learnedLayouts) layouts learned") {
          SettingsButton(title: "Forget", enabled: d.learnedLayouts > 0) { model.act("forget-layouts") }
        }
        .background(DictionaryHighlight(lit: model.highlight == DictionaryRows.layouts))
        .id(DictionaryRows.layouts)
      }
      SettingsCaption(text: learningCaption(d))
      SettingsHeader(title: "Words") {
        SettingsButton(title: "Edit as Text…") { model.act("edit-dictionary") }
      }
      SettingsGroup {
        // By position: a file edited as text can hold the same word twice. Only its first copy
        // edits, the one `Dictionary::edit_word` rewrites.
        ForEach(Array(d.words.enumerated()), id: \.offset) { offset, word in
          let first = d.words.firstIndex(of: word) == offset
          let row = DictionaryRows.word(word)
          Group {
            if model.editing?.kind == .word && model.editing?.was == word && first {
              DictionaryEditRow(edit: editing(.word(word)), lit: model.highlight == row,
                                save: { model.saveEditing(word) }, cancel: { model.editing = nil })
            } else {
              DictionaryEntryRow(title: d.learned.contains(word) ? "\(word) ✨" : word, lit: model.highlight == row,
                                 edit: { model.startEditing(.word(word)) },
                                 remove: { model.act("remove-word", ["word": word]) })
            }
          }
          .id(first ? row : "\(row)#\(offset)")
          SettingsSeparator()
        }
        DictionaryAddRow(placeholder: "Add a word or phrase", enabled: d.error == nil) { model.act("add-word", ["word": $0]) }
      }
      let problem = d.error ?? d.saveError
      SettingsCaption(text: problem ?? "Written exactly as saved. Near misses of words of seven letters or more are fixed too.", error: problem != nil)
      SettingsHeader("Replacements")
      SettingsGroup {
        ForEach(d.replacements, id: \.heard) { entry in
          Group {
            if model.editing?.kind == .replacement && model.editing?.was == entry.heard {
              DictionaryEditRow(edit: editing(.replacement(entry)),
                                save: { model.saveEditing(entry.heard) }, cancel: { model.editing = nil })
            } else {
              DictionaryEntryRow(title: entry.heard, detail: entry.write,
                                 edit: { model.startEditing(.replacement(entry)) },
                                 remove: { model.act("remove-replacement", ["heard": entry.heard]) })
            }
          }
          .id(DictionaryRows.replacement(entry.heard))
          SettingsSeparator()
        }
        ReplacementAddRow(enabled: d.error == nil) { model.act("add-replacement", ["heard": $0, "write": $1]) }
      }
      SettingsCaption(text: "When Scrawl hears the words on the left, it writes the ones on the right.")
    }
  }

  /// The open edit, for its row; `entry` stands in while the row goes away.
  private func editing(_ entry: DictionaryEditing) -> Binding<DictionaryEditing> {
    Binding(
      get: {
        guard let open = model.editing, open.kind == entry.kind, open.was == entry.was else { return entry }
        return open
      },
      set: { if model.editing?.kind == entry.kind && model.editing?.was == entry.was { model.editing = $0 } }
    )
  }

  /// The Pasting group's caption; the formatter's line only when it can't run, since the switch
  /// already says what it does.
  private func learningCaption(_ d: SettingsDictation) -> String {
    var lines = ["Words marked ✨ were learned from your corrections to pasted text."]
    if !d.formattingAvailable {
      lines.append("Paragraphs and lists are off because the formatter isn't set up. Run the installer again to set it up.")
    } else if d.formatting && d.learn {
      lines.append("Paragraphs and lists you rearrange teach the formatter your layout.")
    }
    return lines.joined(separator: " ") + "\nThe microphone and transcription model are set in Recording."
  }

  /// The popup's mode from `lowerPlayback` and `playbackLevel`: a level of 0 mutes.
  private var playbackMode: String {
    !state.lowerPlayback ? "unchanged" : state.playbackLevel == 0 ? "mute" : "lower"
  }

  private func setPlaybackMode(_ mode: String) {
    model.set("lowerPlayback", mode != "unchanged")
    if mode == "mute" {
      model.set("playbackLevel", 0)
    } else if mode == "lower" && state.playbackLevel == 0 {
      model.set("playbackLevel", 20)
    }
  }

  private var recording: some View {
    let t = state.transcriber
    let engines = t.engines.map { (id: $0.id, title: $0.title, enabled: $0.enabled) }
    let chosenModel = t.models.first(where: \.chosen)
    let engineTitle = t.engines.first { $0.id == t.engine }?.title ?? "Apple Speech"
    let mic = state.microphone
    return VStack(spacing: 0) {
      SettingsGroup {
        SettingsRow(title: "Microphone") {
          SettingsPopup(value: mic.value, options: mic.options.map { ($0.id, $0.title, $0.enabled) }, selected: mic.selected) {
            model.set("microphone", $0)
          }
        }
        if mic.meter != nil {
          SettingsSeparator()
          SettingsRow(title: "Input level") { LevelMeter(meter: model.meter) }
        }
      }
      if let note = mic.note {
        SettingsCaption(text: note)
          .transition(.opacity)
      }
      Color.clear.frame(height: 20)
      SettingsGroup {
        SettingsRow(title: "Playback during recording") {
          SettingsPopup(value: playbackMode == "mute" ? "Mute" : playbackMode == "lower" ? "Lower volume" : "Unchanged",
                        options: [("unchanged", "Unchanged", true), ("mute", "Mute", true), ("lower", "Lower volume", true)],
                        selected: playbackMode) { setPlaybackMode($0) }
        }
        if playbackMode == "lower" {
          SettingsSeparator()
          SettingsRow(title: "Relative volume") {
            HStack {
              Slider(value: Binding(get: { Double(model.state.playbackLevel) }, set: {
                model.state.playbackLevel = Int($0.rounded())
                model.set("playbackLevel", Int($0.rounded()))
              }), in: 0...100).frame(width: 150)
              Text("\(state.playbackLevel)%").monospacedDigit().frame(width: 40)
            }
          }
        }
      }
      SettingsCaption(text: "Restores the previous volume after recording. Requires an output device with volume control.")
        .padding(.bottom, 16)
      SettingsGroup {
        SettingsRow(title: "Zoom window captures") {
          SettingsSwitch(on: state.cropWindowCaptures) { model.set("cropWindowCaptures", $0) }
        }
      }
      SettingsCaption(text: "Crops each screenshot to what you worked on in the window. The full frames are kept too.")
        .padding(.bottom, 16)
      SettingsGroup {
        SettingsRow(title: "Transcription") {
          SettingsPopup(value: engineTitle, options: engines, selected: t.engine) {
            model.set("transcriber", $0)
          }
        }
        if t.engine == "hex", !t.models.isEmpty {
          SettingsSeparator()
          SettingsRow(title: "Hex model") {
            SettingsPopup(
              value: chosenModel?.title ?? "Choose",
              options: t.models.map { ($0.id, $0.title, true) },
              selected: chosenModel?.id
            ) { model.set("hexModel", $0) }
          }
        }
      }
      SettingsCaption(text: "Parakeet, Whisper and Apple Speech run on this Mac and time each word. Parakeet and Whisper are enabled once parakeetCommand or whisperCommand is in config.json: install.sh sets up Parakeet, and install.sh --whisper adds Whisper. "
        + (t.hexEnabled || t.engine == "hex"
          ? "Hex uses a model you have in Hex, and its word times are estimates. "
          : "Hex shows up here while Hex is running. ")
        + "Command runs the program in transcribeCommand.")
      if t.engine == "apple", let line = t.speech?.line {
        SettingsCaption(text: line)
      }
      SettingsHeader("Experimental")
      SettingsGroup {
        SettingsRow(title: "Video export") {
          SettingsSwitch(on: state.videoExport) { model.set("videoExport", $0) }
        }
      }
      SettingsCaption(text: "Adds Export Last Capture as Video… to the menu.")
    }
    .animation(SettingsMotion.state, value: mic)
  }

  private var storage: some View {
    let r = state.retention
    let current = r.options.first { $0.id == r.preset }?.label ?? "Custom"
    return VStack(spacing: 0) {
      SettingsGroup {
        SettingsRow(title: "Keep recordings") {
          SettingsPopup(value: current, options: r.options.map { ($0.id, $0.label, true) }, selected: r.preset) {
            model.set("retention", $0)
          }
        }
      }
      if !r.note.isEmpty {
        SettingsCaption(text: r.note)
      }
      SettingsHeader("Captures")
      SettingsGroup {
        SettingsRow(title: "Captures folder") {
          Text(state.capturesSize ?? "Measuring…")
            .font(.system(size: 14).monospacedDigit())
            .foregroundStyle(SettingsTheme(scheme).secondary)
          SettingsButton(title: "Show in Finder") { model.act("show-captures") }
        }
      }
    }
  }

  private var permissions: some View {
    let required = state.permissions.filter(\.required)
    let optional = state.permissions.filter { !$0.required }
    return VStack(spacing: 0) {
      SettingsGroup { permissionRows(required) }
      SettingsCaption(text: "Scrawl needs these three to record.")
      SettingsHeader("When you use them")
      SettingsGroup { permissionRows(optional) }
    }
  }

  @ViewBuilder private func permissionRows(_ rows: [SettingsPermission]) -> some View {
    ForEach(Array(rows.enumerated()), id: \.element.id) { i, row in
      if i > 0 { SettingsSeparator() }
      PermissionLine(row: row, paste: state.pasteLabel, model: model)
    }
  }

  private var agents: some View {
    let theme = SettingsTheme(scheme)
    let cli = state.commandLine
    return VStack(spacing: 0) {
      SettingsGroup {
        HStack(spacing: 12) {
          VStack(alignment: .leading, spacing: 1) {
            Text("Command-line tool").font(.system(size: 14)).foregroundStyle(theme.text)
            if let path = cli.path {
              Text(path)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(theme.secondary)
                .truncationMode(.head)
            }
          }
          .lineLimit(1)
          Spacer(minLength: 8)
          if let button = cli.button {
            SettingsButton(title: button == "uninstall" ? "Uninstall" : "Install", enabled: cli.enabled) {
              model.act(button == "uninstall" ? "uninstall-cli" : "install-cli")
            }
          }
        }
        .padding(.horizontal, 12)
        .frame(height: cli.path == nil ? SettingsMetrics.row : 52)
      }
      SettingsCaption(text: cli.caption, error: cli.error)
        .animation(SettingsMotion.state, value: cli)
      SettingsHeader("Agent skill")
      SettingsGroup { CopyLine(text: state.skillCommand) }
      SettingsCaption(text: "Run it in a terminal. It teaches Claude Code, OpenCode, Codex and other agents to read your captures and use scrawl.")
    }
  }

  private var about: some View {
    let theme = SettingsTheme(scheme)
    return VStack(spacing: 0) {
      SettingsGroup {
        HStack(spacing: 12) {
          if let icon = model.icon {
            Image(nsImage: icon).resizable().interpolation(.high).frame(width: 40, height: 40)
          }
          VStack(alignment: .leading, spacing: 1) {
            Text("Scrawl").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.text)
            Text("Version \(state.version)").font(.system(size: 12).monospacedDigit()).foregroundStyle(theme.secondary)
          }
          Spacer()
          SettingsButton(title: "Check for Updates…", enabled: state.updates) { model.act("check-for-updates") }
        }
        .padding(.horizontal, 12)
        .frame(height: 64)
      }
      if !state.updates {
        SettingsCaption(text: "Updates work in the installed Scrawl.app.")
      }
      SettingsHeader("Links")
      SettingsGroup {
        link("Website", "scrawl.kitlangton.com", "https://scrawl.kitlangton.com")
        SettingsSeparator()
        link("Source", "github.com/kitlangton/scrawl", "https://github.com/kitlangton/scrawl")
        SettingsSeparator()
        SettingsRow(title: "Welcome tour") {
          SettingsButton(title: "Show") { model.act("tour") }
        }
      }
    }
  }

  private func link(_ title: String, _ shown: String, _ url: String) -> some View {
    let theme = SettingsTheme(scheme)
    return Button {
      if let url = URL(string: url) { NSWorkspace.shared.open(url) }
    } label: {
      SettingsRow(title: title) {
        HStack(spacing: 5) {
          Text(shown).font(.system(size: 14)).foregroundStyle(theme.secondary)
          Image(systemName: "arrow.up.right").font(.system(size: 10, weight: .semibold)).foregroundStyle(theme.tertiary)
        }
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
  }
}

/// A command in monospace with a Copy button that says Copied for a moment.
struct CopyLine: View {
  @Environment(\.settingsTheme) private var theme
  let text: String
  @State private var copied = false

  var body: some View {
    HStack(spacing: 12) {
      Text(text)
        .font(.system(size: 12, design: .monospaced))
        .foregroundStyle(theme.text)
        .lineLimit(1)
        .textSelection(.enabled)
      Spacer(minLength: 8)
      SettingsButton(title: copied ? "Copied" : "Copy") {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        copied = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { copied = false }
      }
      .animation(SettingsMotion.state, value: copied)
    }
    .padding(.horizontal, 12)
    .frame(height: SettingsMetrics.row)
  }
}

struct PermissionLine: View {
  @Environment(\.settingsTheme) private var theme
  let row: SettingsPermission
  let paste: String
  @ObservedObject var model: SettingsModel

  var body: some View {
    let info = permissionInfo(row.permission, paste: paste)
    HStack(spacing: 12) {
      IconTile(symbol: info.symbol, tint: tint, size: 26)
      VStack(alignment: .leading, spacing: 1) {
        Text(name).font(.system(size: 14)).foregroundStyle(theme.text)
        Text(why).font(.system(size: 12)).foregroundStyle(theme.secondary)
      }
      .lineLimit(1)
      Spacer(minLength: 8)
      ZStack(alignment: .trailing) {
        if row.granted && row.relaunch {
          SettingsButton(title: "Reopen Scrawl") { model.act("reopen") }
        } else if row.granted {
          HStack(spacing: 5) {
            Image(systemName: "checkmark.circle.fill").foregroundStyle(theme.positive)
            Text("Allowed").foregroundStyle(theme.secondary)
          }
          .font(.system(size: 12, weight: .medium))
        } else {
          SettingsButton(title: row.requested || row.status == "denied" ? "Open Settings" : "Allow") {
            model.act("grant", ["permission": row.permission])
          }
        }
      }
      .animation(SettingsMotion.state, value: row)
    }
    .padding(.horizontal, 12)
    .frame(height: 52)
  }

  /// Shorter than onboarding's, to fit one line beside the button.
  private var name: String {
    switch row.permission {
    case "screen-recording": "Screen Recording"
    case "accessibility": "Accessibility"
    default: permissionInfo(row.permission, paste: paste).name
    }
  }

  private var why: String {
    switch row.permission {
    case "screen-recording": "To see your screen"
    case "microphone": "To hear you"
    case "accessibility": "For ⌘Z and \(paste)"
    case "camera": "To show a cabled iPhone"
    case "speech-recognition": "To write down what you say"
    default: ""
    }
  }

  private var tint: (Color, Color) {
    switch row.permission {
    case "screen-recording": (Color(red: 0.62, green: 0.5, blue: 1), Color(red: 0.44, green: 0.32, blue: 0.92))
    case "microphone": (Color(red: 1, green: 0.62, blue: 0.3), Color(red: 0.95, green: 0.44, blue: 0.12))
    case "accessibility": (Color(red: 0.36, green: 0.66, blue: 1), Color(red: 0.16, green: 0.45, blue: 0.95))
    case "camera": (Color(red: 0.36, green: 0.82, blue: 0.56), Color(red: 0.16, green: 0.66, blue: 0.4))
    case "speech-recognition": (Color(red: 1, green: 0.42, blue: 0.56), Color(red: 0.9, green: 0.24, blue: 0.4))
    default: (Color(white: 0.62), Color(white: 0.45))
    }
  }
}

// MARK: Recording keys

enum ShortcutKeys {
  static func carbon(_ flags: NSEvent.ModifierFlags) -> Int {
    var bits = 0
    if flags.contains(.command) { bits |= cmdKey }
    if flags.contains(.shift) { bits |= shiftKey }
    if flags.contains(.option) { bits |= optionKey }
    if flags.contains(.control) { bits |= controlKey }
    return bits
  }

  /// Whether macOS has this combination as one of its enabled shortcuts (Keyboard Shortcuts).
  static func reserved(keyCode: Int, modifiers: Int) -> Bool {
    var unmanaged: Unmanaged<CFArray>?
    guard CopySymbolicHotKeys(&unmanaged) == noErr,
          let hotkeys = unmanaged?.takeRetainedValue() as? [[String: Any]]
    else { return false }
    let mask = cmdKey | shiftKey | optionKey | controlKey
    return hotkeys.contains { hotkey in
      (hotkey[kHISymbolicHotKeyEnabled as String] as? Bool) == true
        && (hotkey[kHISymbolicHotKeyCode as String] as? Int) == keyCode
        && ((hotkey[kHISymbolicHotKeyModifiers as String] as? Int) ?? -1) & mask == modifiers
    }
  }
}

// MARK: Window

final class SettingsWindow: NSObject, NSApplicationDelegate, NSWindowDelegate {
  static func run(arguments: [String]) -> Never {
    let pane = option("--pane", in: arguments).flatMap(SettingsPane.init(rawValue:)) ?? .general
    let model = SettingsModel(pane: pane, icon: Onboarding.loadIcon(option("--icon", in: arguments)))
    if let dir = option("--render", in: arguments) {
      MainActor.assumeIsolated { render(model: model, to: URL(filePath: dir)) }
      exit(0)
    }
    let app = NSApplication.shared
    let delegate = SettingsWindow(model: model)
    app.setActivationPolicy(.accessory)
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
    exit(0)
  }

  let model: SettingsModel
  var window: NSWindow?
  private lazy var keys = HelperWindowKeys(
    window: { [weak self] in self?.window },
    enabled: { [weak self] in self?.model.state.recording == nil }
  )
  private var monitor: Any?
  private var presented = false
  private var paneWatch: AnyCancellable?

  init(model: SettingsModel) {
    self.model = model
  }

  func applicationDidFinishLaunching(_: Notification) {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: SettingsMetrics.width, height: SettingsMetrics.height),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    // A toolbar makes the title bar as tall as the pane titles, so the traffic lights share their line.
    let toolbar = NSToolbar(identifier: "scrawl-settings")
    toolbar.showsBaselineSeparator = false
    window.toolbar = toolbar
    window.toolbarStyle = .unified
    window.titlebarAppearsTransparent = true
    window.titleVisibility = .hidden
    window.title = "Scrawl Settings"
    window.isMovableByWindowBackground = true
    window.isReleasedWhenClosed = false
    window.delegate = self
    let host = FirstClickHostingView(rootView: SettingsView(model: model))
    // The panes draw under the title bar themselves (the sidebar holds the traffic lights).
    host.safeAreaRegions = []
    host.sizingOptions = []
    window.contentView = host
    // With a toolbar, the content size leaves out the title bar; the window is exactly the panes.
    window.setFrame(NSRect(x: 0, y: 0, width: SettingsMetrics.width, height: SettingsMetrics.height), display: false)
    host.frame = window.contentLayoutRect.union(NSRect(origin: .zero, size: window.frame.size))
    host.autoresizingMask = [.width, .height]
    window.minSize = NSSize(width: SettingsMetrics.minWidth, height: SettingsMetrics.minHeight)
    if !window.setFrameUsingName("ScrawlSettings") { window.center() }
    // Saved frames can predate the minimum size. Clamp them explicitly too.
    var restored = window.frame
    restored.size.width = max(restored.width, SettingsMetrics.minWidth)
    restored.size.height = max(restored.height, SettingsMetrics.minHeight)
    window.setFrame(restored, display: false)
    window.setFrameAutosaveName("ScrawlSettings")
    self.window = window
    _ = keys
    model.send = { emit($0) }
    paneWatch = model.$pane.sink { [weak self] pane in self?.updateMeter(pane: pane) }
    readCommands()
  }

  func windowWillResize(_ sender: NSWindow, to frameSize: NSSize) -> NSSize {
    NSSize(width: max(frameSize.width, SettingsMetrics.minWidth),
           height: max(frameSize.height, SettingsMetrics.minHeight))
  }

  func windowDidChangeOcclusionState(_: Notification) {
    updateMeter()
  }

  /// The level shows only while someone can see it: the Recording pane, on screen.
  private func updateMeter(pane: SettingsPane? = nil) {
    let onScreen = window?.occlusionState.contains(.visible) == true && window?.isMiniaturized == false
    let showing = (pane ?? model.pane) == .recording && onScreen
    model.meter.meter(showing ? model.state.microphone.meter : nil)
  }

  func windowWillClose(_: Notification) {
    emit(["action": "close"])
    keys.handBack()
    exit(0)
  }

  func windowDidResignKey(_: Notification) {
    if model.state.recording != nil { model.act("cancel-shortcut") }
  }

  private func present(_ pane: SettingsPane?) {
    if let pane { model.pane = pane }
    keys.bringForward(window)
  }

  /// Listens for keys only while a recorder is waiting for one.
  private func listen(_ on: Bool) {
    if on, monitor == nil {
      model.liveModifiers = []
      monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .flagsChanged]) { [weak self] event in
        guard let self, let kind = self.model.state.recording else { return event }
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if event.type == .flagsChanged {
          self.model.liveModifiers = flags
          return nil
        }
        if event.keyCode == UInt16(kVK_Escape) {
          self.model.act("cancel-shortcut")
          return nil
        }
        let modifiers = ShortcutKeys.carbon(flags)
        let keyCode = Int(event.keyCode)
        self.model.act("shortcut", [
          "kind": kind,
          "keyCode": keyCode,
          "modifiers": modifiers,
          "key": event.characters(byApplyingModifiers: []) ?? "",
          "reserved": ShortcutKeys.reserved(keyCode: keyCode, modifiers: modifiers),
        ])
        return nil
      }
    } else if !on, let monitor {
      NSEvent.removeMonitor(monitor)
      self.monitor = nil
      model.liveModifiers = []
    }
  }

  private func readCommands() {
    Thread.detachNewThread {
      while let line = readLine() {
        guard let data = line.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = object["type"] as? String
        else { continue }
        let permission = object["permission"] as? String ?? ""
        let pane = (object["pane"] as? String).flatMap(SettingsPane.init(rawValue:))
        let focus = object["focus"] as? [String: Any]
        let state = type == "state" ? try? JSONDecoder().decode(SettingsState.self, from: data) : nil
        DispatchQueue.main.async {
          switch type {
          case "state":
            guard let state else { return }
            self.model.state = state
            self.listen(state.recording != nil)
            self.updateMeter()
            if !self.presented {
              self.presented = true
              self.present(nil)
            }
          case "show":
            self.present(pane)
            if let focus = focus { self.model.focus(focus) }
          case "request": Permissions.request(permission)
          case "open-settings": Permissions.openSettings(permission)
          case "close":
            self.keys.handBack()
            exit(0)
          default: break
          }
        }
      }
      exit(0)
    }
  }

  /// Every pane, dark and light, plus the recorder listening and a refused shortcut, as PNGs.
  @MainActor static func render(model: SettingsModel, to dir: URL) {
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    model.rendering = true
    var base = SettingsState()
    base.capturesSize = "1.4 GB"
    base.permissions = base.permissions.map { row in
      var row = row
      row.status = ["screen-recording", "microphone", "accessibility"].contains(row.permission) ? "granted" : "not-determined"
      if row.permission == "camera" { row.status = "denied" }
      return row
    }
    let devices = [
      SettingsMenuOption(id: "BuiltInMicrophoneDevice", title: "MacBook Pro Microphone"),
      SettingsMenuOption(id: "AppleUSBAudioEngine:Shure:MV7", title: "Shure MV7"),
    ]
    let micOptions = [SettingsMenuOption(id: "default", title: "System default (MacBook Pro Microphone)"), SettingsMenuOption(id: "-", title: "", enabled: false)] + devices
    base.microphone = SettingsMicrophone(selected: "default", value: "System default", options: micOptions, note: nil, meter: "BuiltInMicrophoneDevice")
    var listening = base
    listening.shortcuts[0].recording = true
    var refused = base
    refused.shortcuts[1].error = "Apps use ⌘V. Add ⌥ or ⌃."
    var custom = base
    custom.shortcuts[0] = SettingsShortcut(kind: "record", title: "Record", label: "⌃⌥⌘R", key: "R", shift: false, control: true, option: true, command: true, isDefault: false)
    custom.login.on = true
    var lower = base
    lower.playbackLevel = 20
    var hex = base
    hex.transcriber = SettingsTranscriber(engine: "hex", hexTitle: "Hex", hexEnabled: true, models: [
      SettingsModelOption(id: "parakeet-tdt-0.6b-v3", title: "Parakeet TDT v3", chosen: true),
      SettingsModelOption(id: "whisper-large-v3-turbo", title: "Whisper Large v3 Turbo", chosen: false),
    ])
    hex.videoExport = true
    var downloading = base
    downloading.transcriber.speech = SpeechStatus(state: "downloading", locale: "en_US", progress: 0.42)
    var missing = base
    missing.permissions[0].status = "granted"
    missing.permissions[0].relaunch = true
    missing.permissions[2].status = "not-determined"
    missing.permissions[2].requested = true
    var noUpdates = base
    noUpdates.updates = false
    var cliInstalled = base
    cliInstalled.commandLine = SettingsCommandLine(
      path: "/opt/homebrew/bin/scrawl", button: "uninstall",
      caption: "Agents and your terminal can run scrawl to read captures, zoom into a moment and export video.")
    var cliLocal = base
    cliLocal.commandLine = SettingsCommandLine(
      path: "~/.local/bin/scrawl", button: "uninstall",
      caption: "If your shell can't find scrawl, add ~/.local/bin to your PATH.")
    var cliHomebrew = base
    cliHomebrew.commandLine = SettingsCommandLine(
      path: "/opt/homebrew/bin/scrawl", button: nil,
      caption: "Installed with Homebrew. Agents and your terminal can run scrawl to read captures, zoom into a moment and export video.")
    var cliTaken = base
    cliTaken.commandLine = SettingsCommandLine(
      button: nil, caption: "/usr/local/bin/scrawl is another program, so Scrawl leaves it alone.", error: true)
    var cliDev = base
    cliDev.commandLine = SettingsCommandLine(
      enabled: false, caption: "The command-line tool works in the installed Scrawl.app.")
    var micChosen = base
    micChosen.microphone = SettingsMicrophone(selected: devices[1].id, value: "Shure MV7", options: micOptions, note: nil, meter: devices[1].id)
    var micMissing = base
    micMissing.microphone = SettingsMicrophone(
      selected: "airpods", value: "AirPods Pro",
      options: micOptions + [SettingsMenuOption(id: "airpods", title: "AirPods Pro (not connected)", enabled: false)],
      note: "AirPods Pro isn't connected. Using MacBook Pro Microphone.", meter: devices[0].id
    )
    var micNotAllowed = base
    micNotAllowed.microphone.meter = nil
    var dictating = base
    dictating.dictation.on = true
    dictating.dictation.words = ["Ferris", "Varde Labs", "Parakeet", "Klose"]
    dictating.dictation.learned = ["Klose"]
    dictating.dictation.learnedLayouts = 3
    dictating.dictation.replacements = [
      SettingsReplacement(heard: "feris", write: "Ferris"),
      SettingsReplacement(heard: "var de labs", write: "Varde Labs"),
    ]
    var editingWord = dictating
    editingWord.dictation.words = ["Atlas", "Nova", "Parakeet", "Orbit"]
    editingWord.dictation.learned = ["Orbit"]
    editingWord.dictation.replacements = [
      SettingsReplacement(heard: "atlis", write: "Atlas"),
      SettingsReplacement(heard: "no va", write: "Nova"),
    ]
    let edits: [String: DictionaryEditing] = [
      "dictation-editing": DictionaryEditing(kind: .word, was: "Nova", text: "Nova Prime"),
      "dictation-edit-refused": DictionaryEditing(kind: .word, was: "Orbit", text: "atlas", refusal: "That's already in your dictionary."),
      "dictation-editing-replacement": DictionaryEditing(kind: .replacement, was: "atlis", text: "atless", write: "Atlas"),
    ]
    var replacementsOnly = dictating
    replacementsOnly.dictation.words = []
    var dictionaryBroken = base
    dictionaryBroken.dictation.error = "~/.config/scrawl/dictionary.json isn't a valid dictionary. Fix it with Edit as Text…; Scrawl won't change it until then."
    let samples: [(String, SettingsPane, SettingsState, NSEvent.ModifierFlags)] = [
      ("general", .general, base, []),
      ("general-custom", .general, custom, []),
      ("general-recording-shortcut", .general, listening, [.control, .option]),
      ("general-refused", .general, refused, []),
      ("recording", .recording, base, []),
      ("recording-lower", .recording, lower, []),
      ("recording-hex", .recording, hex, []),
      ("recording-mic-chosen", .recording, micChosen, []),
      ("recording-mic-missing", .recording, micMissing, []),
      ("recording-mic-not-allowed", .recording, micNotAllowed, []),
      ("recording-downloading", .recording, downloading, []),
      ("dictation", .dictation, dictating, []),
      ("dictation-empty", .dictation, base, []),
      ("dictation-replacements", .dictation, replacementsOnly, []),
      ("dictation-broken", .dictation, dictionaryBroken, []),
      ("dictation-editing", .dictation, editingWord, []),
      ("dictation-edit-refused", .dictation, editingWord, []),
      ("dictation-editing-replacement", .dictation, editingWord, []),
      ("dictation-opened", .dictation, editingWord, []),
      ("storage", .storage, base, []),
      ("permissions", .permissions, base, []),
      ("permissions-missing", .permissions, missing, []),
      ("agents", .agents, base, []),
      ("agents-installed", .agents, cliInstalled, []),
      ("agents-local", .agents, cliLocal, []),
      ("agents-homebrew", .agents, cliHomebrew, []),
      ("agents-taken", .agents, cliTaken, []),
      ("agents-dev", .agents, cliDev, []),
      ("about", .about, base, []),
      ("about-dev", .about, noUpdates, []),
    ]
    var index = 0
    for scheme in [ColorScheme.dark, .light] {
      for (name, pane, state, live) in samples {
        if scheme == .light && name.contains("-") && name != "general-recording-shortcut" && !name.hasPrefix("recording-mic")
          && !name.hasPrefix("dictation-edit") && name != "dictation-opened" { continue }
        index += 1
        model.state = state
        model.pane = pane
        // The Words and Replacements groups sit below the window's height.
        let editing = edits[name] ?? (name == "dictation-opened" ? .word("Orbit") : nil)
        model.editing = editing
        model.highlight = name == "dictation-opened" ? DictionaryRows.word("Orbit") : nil
        model.renderScroll = editing == nil ? 0 : 560
        model.liveModifiers = live
        model.meter.level = 0.44
        let view = SettingsView(model: model)
          .frame(width: SettingsMetrics.width, height: SettingsMetrics.height)
          .environment(\.colorScheme, scheme)
          .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        let renderer = ImageRenderer(content: view)
        renderer.scale = 2
        guard let image = renderer.cgImage else { continue }
        let rep = NSBitmapImageRep(cgImage: image)
        let suffix = scheme == .dark ? "dark" : "light"
        let url = dir.appending(path: String(format: "%02d-%@-%@.png", index, name, suffix))
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
      }
    }
  }
}
