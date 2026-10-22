// The fork's updater (`scrawl-helper fork-update --repo OWNER/NAME`), which Check for Updates…
// runs in place of Sparkle: Sparkle reads Kit's feed and would replace the fork with his build.
// It compares the newest GitHub release with the running build's `ScrawlForkCommit`, shows the
// release notes, downloads the release zip, checks it against the release's `.sha256` asset,
// and runs the zip's `install.sh` on its own: the installer quits and reopens Scrawl.
// `--self-test` checks the parsing and the version decision without the network.

import AppKit
import CryptoKit
import SwiftUI

struct ForkAsset: Equatable {
  var name: String
  var url: URL
}

struct ForkRelease: Equatable {
  var tag: String
  var name: String
  var notes: String
  var zip: ForkAsset
  var checksum: ForkAsset

  /// The newest release's JSON from the GitHub API, when it has a `Scrawl-Fork-*.zip` and the
  /// `.sha256` beside it.
  static func parse(_ data: Data) -> ForkRelease? {
    guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let tag = object["tag_name"] as? String,
          let rawAssets = object["assets"] as? [[String: Any]]
    else { return nil }
    let assets = rawAssets.compactMap { asset -> ForkAsset? in
      guard let name = asset["name"] as? String,
            let link = asset["browser_download_url"] as? String,
            let url = URL(string: link), url.scheme == "https", url.host == "github.com"
      else { return nil }
      return ForkAsset(name: name, url: url)
    }
    let pairs = assets.lazy
      .filter { $0.name.hasPrefix("Scrawl-Fork-") && $0.name.hasSuffix(".zip") }
      .compactMap { zip in assets.first { $0.name == zip.name + ".sha256" }.map { (zip, $0) } }
    guard let (zip, checksum) = pairs.first else { return nil }
    return ForkRelease(
      tag: tag, name: object["name"] as? String ?? tag, notes: object["body"] as? String ?? "",
      zip: zip, checksum: checksum)
  }
}

enum ForkVersion {
  /// The digest a `shasum -a 256` line gives for `file`, lowercased; nil for any other file.
  static func digest(inChecksum text: String, for file: String) -> String? {
    for line in text.split(separator: "\n") {
      let parts = line.split(separator: " ", maxSplits: 1)
      guard parts.count == 2 else { continue }
      let digest = parts[0].lowercased()
      let name = parts[1].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "*"))
      if name == file, digest.count == 64, digest.allSatisfy(\.isHexDigit) { return digest }
    }
    return nil
  }

  /// Whether a release is worth offering, from GitHub's compare status of this build against it
  /// (nil when the build has no commit stamp or GitHub doesn't know its commit).
  static func newer(compareStatus: String?) -> Bool {
    switch compareStatus {
    case "identical", "behind": false
    default: true
    }
  }

  static func selfTest() {
    let release = """
      {"tag_name":"fork-abc1234","name":"Scrawl fork abc1234","body":"Notes","assets":[
        {"name":"Scrawl-Fork-abc1234.zip","browser_download_url":"https://github.com/o/r/releases/download/t/z.zip"},
        {"name":"Scrawl-Fork-abc1234.zip.sha256","browser_download_url":"https://github.com/o/r/releases/download/t/z.sha256"}]}
      """
    let parsed = ForkRelease.parse(Data(release.utf8))
    precondition(parsed?.zip.name == "Scrawl-Fork-abc1234.zip" && parsed?.checksum.url.lastPathComponent == "z.sha256")
    precondition(ForkRelease.parse(Data(release.replacingOccurrences(of: ".zip.sha256", with: ".txt").utf8)) == nil)
    let lone = #"{"name":"Scrawl-Fork-old.zip","browser_download_url":"https://github.com/o/r/releases/download/t/old.zip"},"#
    precondition(ForkRelease.parse(Data(release.replacingOccurrences(of: #""assets":["#, with: #""assets":["# + lone).utf8))?.zip.name == "Scrawl-Fork-abc1234.zip")
    precondition(ForkRelease.parse(Data(release.replacingOccurrences(of: "https://", with: "http://").utf8)) == nil)
    precondition(ForkRelease.parse(Data(release.replacingOccurrences(of: "github.com", with: "example.com").utf8)) == nil)
    let digest = String(repeating: "ab", count: 32)
    precondition(self.digest(inChecksum: "\(digest)  Scrawl-Fork-abc1234.zip\n", for: "Scrawl-Fork-abc1234.zip") == digest)
    precondition(self.digest(inChecksum: "\(digest.uppercased()) *Scrawl-Fork-abc1234.zip", for: "Scrawl-Fork-abc1234.zip") == digest)
    precondition(self.digest(inChecksum: "\(digest)  other.zip", for: "Scrawl-Fork-abc1234.zip") == nil)
    precondition(self.digest(inChecksum: "abc  Scrawl-Fork-abc1234.zip", for: "Scrawl-Fork-abc1234.zip") == nil)
    precondition(newer(compareStatus: "ahead") && newer(compareStatus: "diverged") && newer(compareStatus: nil))
    precondition(!newer(compareStatus: "identical") && !newer(compareStatus: "behind"))
    emit(["selfTest": "ok"])
  }
}

@MainActor
final class ForkUpdater: NSObject, ObservableObject, URLSessionDownloadDelegate {
  enum Phase: Equatable {
    case checking
    case upToDate
    case available(ForkRelease)
    case downloading(Double)
    case verifying
    case installing(log: String)
    case failed(String)
  }

  @Published var phase = Phase.checking
  /// GitHub's compare status of this build against the offered release, when it has one.
  @Published var relation: String?
  let repo: String
  /// The enclosing Scrawl.app's `ScrawlForkCommit`: this helper embeds an Info.plist of its own,
  /// so it reads the app's file rather than trusting `Bundle.main` to find it.
  let current: String? = {
    let contents = URL(filePath: CommandLine.arguments[0]).resolvingSymlinksInPath()
      .deletingLastPathComponent().deletingLastPathComponent()
    let plist = NSDictionary(contentsOf: contents.appending(path: "Info.plist"))
    return plist?["ScrawlForkCommit"] as? String
  }()
  private var download: URLSessionDownloadTask?
  private var downloaded: CheckedContinuation<URL, Error>?

  init(repo: String) { self.repo = repo }

  private func api(_ path: String) -> URLRequest {
    var request = URLRequest(url: URL(string: "https://api.github.com/repos/\(repo)/\(path)")!)
    request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
    request.setValue("Scrawl-fork-updater", forHTTPHeaderField: "User-Agent")
    request.timeoutInterval = 20
    return request
  }

  private func fetch(_ request: URLRequest) async throws -> (Data, Int) {
    let (data, response) = try await URLSession.shared.data(for: request)
    return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
  }

  func check() async {
    phase = .checking
    do {
      let (data, status) = try await fetch(api("releases/latest"))
      guard status == 200 else { throw HelperError("GitHub answered \(status) for the latest release.") }
      guard let release = ForkRelease.parse(data) else {
        throw HelperError("The latest release has no Scrawl-Fork zip with a .sha256 beside it.")
      }
      var compare: String?
      if let current {
        let (body, status) = try await fetch(api("compare/\(current)...\(release.tag)"))
        // 404 means GitHub doesn't have this build's commit; any other failure (a rate limit)
        // must not read as "can't tell", which would offer a possibly older release.
        if status == 200, let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any] {
          compare = object["status"] as? String
        } else if status != 404 {
          throw HelperError("GitHub answered \(status) when comparing this build with \(release.tag).")
        }
      }
      emit(["event": "checked", "tag": release.tag, "compare": compare ?? "unknown"])
      relation = compare
      phase = ForkVersion.newer(compareStatus: compare) ? .available(release) : .upToDate
    } catch {
      fail(error)
    }
  }

  func install(_ release: ForkRelease) async {
    guard case .available = phase else { return }
    phase = .downloading(0)
    let work = FileManager.default.temporaryDirectory.appending(path: "scrawl-update-\(UUID().uuidString)")
    do {
      try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
      let (checksumData, status) = try await fetch(URLRequest(url: release.checksum.url))
      guard status == 200, let expected = ForkVersion.digest(
        inChecksum: String(decoding: checksumData, as: UTF8.self), for: release.zip.name)
      else { throw HelperError("The release's checksum file doesn't name \(release.zip.name).") }

      let zip = work.appending(path: release.zip.name)
      try FileManager.default.moveItem(at: try await fetchFile(release.zip.url), to: zip)

      phase = .verifying
      let digest = try await Task.detached { try ForkUpdater.sha256(of: zip) }.value
      guard digest == expected else {
        throw HelperError("The download doesn't match the release's checksum, so it wasn't installed.")
      }
      try await Task.detached { try ForkUpdater.run("/usr/bin/ditto", ["-x", "-k", zip.path, work.path]) }.value
      let installer = work.appending(path: (release.zip.name as NSString).deletingPathExtension).appending(path: "install.sh")
      guard FileManager.default.fileExists(atPath: installer.path) else {
        throw HelperError("The release zip has no install.sh.")
      }
      let log = FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Logs/Scrawl/fork-update.log")
      try launchDetached(installer, log: log)
      emit(["event": "installing", "tag": release.tag])
      phase = .installing(log: log.path)
    } catch {
      try? FileManager.default.removeItem(at: work)
      fail(error)
    }
  }

  private func fail(_ error: Error) {
    let message = (error as? HelperError)?.errorDescription ?? error.localizedDescription
    emit(["event": "failed", "reason": message])
    phase = .failed(message)
  }

  private func fetchFile(_ url: URL) async throws -> URL {
    try await withCheckedThrowingContinuation { continuation in
      downloaded = continuation
      let session = URLSession(configuration: .ephemeral, delegate: self, delegateQueue: .main)
      download = session.downloadTask(with: url)
      download?.resume()
      session.finishTasksAndInvalidate()
    }
  }

  func cancelDownload() { download?.cancel() }

  nonisolated func urlSession(_: URLSession, downloadTask: URLSessionDownloadTask, didWriteData _: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
    guard totalBytesExpectedToWrite > 0 else { return }
    let fraction = Double(totalBytesWritten) / Double(totalBytesExpectedToWrite)
    MainActor.assumeIsolated { phase = .downloading(fraction) }
  }

  nonisolated func urlSession(_: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
    // The system deletes `location` when this returns, so it moves somewhere it keeps first.
    let kept = FileManager.default.temporaryDirectory.appending(path: "scrawl-update-\(UUID().uuidString).zip")
    let moved = Result { try FileManager.default.moveItem(at: location, to: kept) }
    let status = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? 0
    MainActor.assumeIsolated {
      switch (moved, status) {
      case (.success, 200): downloaded?.resume(returning: kept)
      case (.failure(let error), _): downloaded?.resume(throwing: error)
      default:
        try? FileManager.default.removeItem(at: kept)
        downloaded?.resume(throwing: HelperError("GitHub answered \(status) for the download."))
      }
      downloaded = nil
    }
  }

  nonisolated func urlSession(_: URLSession, task _: URLSessionTask, didCompleteWithError error: Error?) {
    guard let error else { return }
    MainActor.assumeIsolated {
      downloaded?.resume(throwing: (error as? URLError)?.code == .cancelled ? HelperError("The download was cancelled.") : error)
      downloaded = nil
    }
  }

  nonisolated private static func sha256(of file: URL) throws -> String {
    let handle = try FileHandle(forReadingFrom: file)
    defer { try? handle.close() }
    var hasher = SHA256()
    while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty { hasher.update(data: chunk) }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
  }

  nonisolated private static func run(_ tool: String, _ arguments: [String]) throws {
    let process = Process()
    process.executableURL = URL(filePath: tool)
    process.arguments = arguments
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { throw HelperError("\(tool) failed while unpacking the release.") }
  }

  /// Starts `install.sh` in the background under `nohup`, so it outlives this helper and Scrawl,
  /// which it quits by name. A GUI app's PATH has no Homebrew, which the installer needs.
  private func launchDetached(_ installer: URL, log: URL) throws {
    try FileManager.default.createDirectory(at: log.deletingLastPathComponent(), withIntermediateDirectories: true)
    let process = Process()
    process.executableURL = URL(filePath: "/bin/sh")
    process.arguments = ["-c", "exec /usr/bin/nohup /bin/bash \"$1\" >\"$2\" 2>&1 </dev/null &", "sh", installer.path, log.path]
    process.currentDirectoryURL = installer.deletingLastPathComponent()
    var environment = ProcessInfo.processInfo.environment
    environment["PATH"] = "/opt/homebrew/bin:/usr/local/bin:" + (environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin")
    process.environment = environment
    try process.run()
    process.waitUntilExit()
  }
}

struct ForkUpdateView: View {
  @ObservedObject var updater: ForkUpdater

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text(title).font(.system(size: 15, weight: .semibold))
      switch updater.phase {
      case .checking, .verifying:
        ProgressView().controlSize(.small)
      case .downloading(let fraction):
        ProgressView(value: fraction)
      case .available(let release):
        ScrollView {
          Text(release.notes.isEmpty ? "No release notes." : release.notes)
            .font(.system(size: 12))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(height: 180)
      case .installing(let log):
        Text("Scrawl quits and opens again when the install finishes. If it doesn't reopen, the installer's output is in \(log).")
          .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
      case .failed(let message):
        Text(message).font(.system(size: 12)).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
      case .upToDate:
        Text(updater.current.map { "This build is \($0.prefix(7))." } ?? "")
          .font(.system(size: 12)).foregroundStyle(.secondary)
      }
      HStack {
        Spacer()
        buttons
      }
    }
    .padding(20)
    .frame(width: 440)
  }

  private var title: String {
    switch updater.phase {
    case .checking: "Checking for updates…"
    case .upToDate: "Scrawl is up to date."
    case .available(let release):
      switch updater.relation {
      case "ahead": "\(release.name) is available."
      case "diverged": "\(release.name) is available. It isn't built on this version, so it may lack changes this build has."
      default: "\(release.name) is available. This build can't be matched to a release, so it may be older than this build."
      }
    case .downloading(let fraction): "Downloading… \(Int(fraction * 100))%"
    case .verifying: "Checking the download…"
    case .installing: "Installing…"
    case .failed: "The update didn't work."
    }
  }

  @ViewBuilder private var buttons: some View {
    switch updater.phase {
    case .available(let release):
      Button("Later") { NSApp.terminate(nil) }.keyboardShortcut(.cancelAction)
      Button("Install") { Task { await updater.install(release) } }.keyboardShortcut(.defaultAction)
    case .downloading:
      Button("Cancel") { updater.cancelDownload() }.keyboardShortcut(.cancelAction)
    case .failed:
      Button("Close") { NSApp.terminate(nil) }.keyboardShortcut(.cancelAction)
      Button("Try Again") { Task { await updater.check() } }.keyboardShortcut(.defaultAction)
    case .checking, .verifying:
      Button("Close") { NSApp.terminate(nil) }.keyboardShortcut(.cancelAction)
    case .upToDate, .installing:
      Button("OK") { NSApp.terminate(nil) }.keyboardShortcut(.defaultAction)
    }
  }
}

enum ForkUpdate {
  @MainActor static func run(arguments: [String]) {
    if arguments.contains("--self-test") {
      ForkVersion.selfTest()
      exit(0)
    }
    if let dir = option("--render", in: arguments) {
      render(to: URL(filePath: dir))
      exit(0)
    }
    guard let repo = option("--repo", in: arguments), repo.split(separator: "/").count == 2 else {
      fail("fork-update needs --repo OWNER/NAME")
    }
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let updater = ForkUpdater(repo: repo)
    let controller = NSHostingController(rootView: ForkUpdateView(updater: updater))
    controller.sizingOptions = [.preferredContentSize]
    let window = NSWindow(contentViewController: controller)
    window.styleMask = [.titled, .closable]
    window.title = "Scrawl Update"
    window.isReleasedWhenClosed = false
    window.center()
    let closer = WindowCloser()
    window.delegate = closer
    window.makeKeyAndOrderFront(nil)
    app.activate(ignoringOtherApps: true)
    Task { await updater.check() }
    withExtendedLifetime(closer) { app.run() }
  }
}

extension ForkUpdate {
  /// Each state of the window as a PNG, for review without the network.
  @MainActor static func render(to dir: URL) {
    let asset = ForkAsset(name: "Scrawl-Fork-abc1234.zip", url: URL(string: "https://example.com/z.zip")!)
    let release = ForkRelease(
      tag: "fork-abc1234", name: "Scrawl fork abc1234",
      notes: "A prebuilt Scrawl for macOS 15 (Apple silicon).\n\n- Settings keeps the fork's window.\n- Check for Updates installs fork releases.",
      zip: asset, checksum: asset)
    let phases: [(String, ForkUpdater.Phase)] = [
      ("checking", .checking), ("up-to-date", .upToDate), ("available", .available(release)),
      ("downloading", .downloading(0.42)), ("verifying", .verifying),
      ("installing", .installing(log: "~/Library/Logs/Scrawl/fork-update.log")),
      ("failed", .failed("The download doesn't match the release's checksum, so it wasn't installed.")),
    ]
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    for (name, phase) in phases {
      let updater = ForkUpdater(repo: "owner/name")
      updater.phase = phase
      let renderer = ImageRenderer(content: ForkUpdateView(updater: updater).background(Color(nsColor: .windowBackgroundColor)))
      renderer.scale = 2
      if let image = renderer.nsImage, let tiff = image.tiffRepresentation,
         let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
        try? png.write(to: dir.appending(path: "fork-update-\(name).png"))
      }
    }
  }
}

/// Quits the helper when its only window closes.
final class WindowCloser: NSObject, NSWindowDelegate {
  func windowWillClose(_: Notification) { NSApp.terminate(nil) }
}
