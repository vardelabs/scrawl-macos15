@preconcurrency import AVFoundation
import CoreGraphics
import Speech

/// On-device transcription with macOS 26's SpeechAnalyzer: finalized results only, each word with
/// its own audio time range. Older systems get an error, and Scrawl keeps the recording without a
/// transcript. `SCRAWL_MACOS26_SDK` is set by `build.rs` when the SDK declares these types.
enum Transcriber {
  static let unavailable = HelperError("SpeechAnalyzer needs macOS 26 or later")

  static func transcribe(_ url: URL) async throws -> [String: Any] {
    #if SCRAWL_MACOS26_SDK
    guard #available(macOS 26, *) else { throw unavailable }
    var found = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current)
    if found == nil { found = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: "en-US")) }
    guard let locale = found else { throw HelperError("SpeechTranscriber has no supported locale") }
    let probeTranscriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [.audioTimeRange])
    if let request = try await AssetInventory.assetInstallationRequest(supporting: [probeTranscriber]) {
      try await request.downloadAndInstall()
    }
    if let chunks = try? splitWavAtSilences(url), chunks.files.count > 1 {
      defer { try? FileManager.default.removeItem(at: chunks.dir) }
      let ordered = try await withThrowingTaskGroup(of: (Int, [[String: Any]], [[String: Any]]).self) { group in
        for (idx, item) in chunks.files.enumerated() {
          group.addTask {
            let (segs, wrds) = try await transcribeSingle(item.url, locale: locale, offsetMs: item.offsetMs)
            return (idx, segs, wrds)
          }
        }
        var collected: [(Int, [[String: Any]], [[String: Any]])] = []
        for try await res in group {
          collected.append(res)
        }
        return collected.sorted { $0.0 < $1.0 }
      }
      var segments: [[String: Any]] = []
      var words: [[String: Any]] = []
      for (_, segs, wrds) in ordered {
        segments.append(contentsOf: segs)
        words.append(contentsOf: wrds)
      }
      return [
        "locale": locale.identifier,
        "text": segments.compactMap { $0["text"] as? String }.joined(separator: " "),
        "segments": segments,
        "words": words,
      ]
    }
    let (segments, words) = try await transcribeSingle(url, locale: locale, offsetMs: 0)
    return [
      "locale": locale.identifier,
      "text": segments.compactMap { $0["text"] as? String }.joined(separator: " "),
      "segments": segments,
      "words": words,
    ]
    #else
    throw unavailable
    #endif
  }

  #if SCRAWL_MACOS26_SDK
  @available(macOS 26, *)
  private static func transcribeSingle(_ url: URL, locale: Locale, offsetMs: Int) async throws -> ([[String: Any]], [[String: Any]]) {
    let transcriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [.audioTimeRange])
    let file = try AVAudioFile(forReading: url)
    let analyzer = try await SpeechAnalyzer(inputAudioFile: file, modules: [transcriber], finishAfterFile: true)
    var segments: [[String: Any]] = []
    var words: [[String: Any]] = []
    for try await result in transcriber.results {
      let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
      guard !text.isEmpty else { continue }
      for run in result.text.runs {
        guard let range = run.audioTimeRange else { continue }
        let word = String(result.text[run.range].characters).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !word.isEmpty else { continue }
        words.append(["t0": ms(range.start) + offsetMs, "t1": ms(range.end) + offsetMs, "text": word])
      }
      segments.append(["t0": ms(result.range.start) + offsetMs, "t1": ms(result.range.end) + offsetMs, "text": text])
    }
    withExtendedLifetime(analyzer) {}
    return (segments, words)
  }
  #endif

  private struct WavChunks {
    let dir: URL
    let files: [(offsetMs: Int, url: URL)]
  }

  /// Splits a 16 kHz mono 16-bit WAV longer than 22 s at silence midpoints (>= 300 ms) so
  /// long recordings transcribe across concurrent SpeechAnalyzer instances without cutting words.
  private static func splitWavAtSilences(_ url: URL) throws -> WavChunks? {
    let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
    guard let fileSize = attrs[.size] as? Int, fileSize >= 22 * 32000 + 44 else { return nil }
    let data = try Data(contentsOf: url)
    guard data.count >= 44 else { return nil }
    let pcmStart = 44
    let pcmBytes = data.count - pcmStart
    let sampleCount = pcmBytes / 2
    let durationMs = sampleCount / 16
    guard durationMs >= 22000 else { return nil }

    let windowSamples = 800 // 50 ms at 16 kHz
    let thresholdSq = (0.012 * 32768.0) * (0.012 * 32768.0)
    var silences: [(midMs: Int, lenMs: Int)] = []
    data.withUnsafeBytes { rawBuf in
      let base = rawBuf.baseAddress!.advanced(by: pcmStart).assumingMemoryBound(to: Int16.self)
      var silenceStartMs: Int? = nil
      var i = 0
      while i + windowSamples <= sampleCount {
        var sumSq: Double = 0
        for j in 0..<windowSamples {
          let v = Double(Int16(littleEndian: base[i + j]))
          sumSq += v * v
        }
        let meanSq = sumSq / Double(windowSamples)
        let tMs = i / 16
        if meanSq <= thresholdSq {
          if silenceStartMs == nil { silenceStartMs = tMs }
        } else if let s0 = silenceStartMs {
          if tMs - s0 >= 300 {
            silences.append(((s0 + tMs) / 2, tMs - s0))
          }
          silenceStartMs = nil
        }
        i += windowSamples
      }
    }
    guard !silences.isEmpty else { return nil }
    let targetChunks = min(6, max(2, (durationMs + 19999) / 20000))
    var cuts = [0]
    for k in 1..<targetChunks {
      let ideal = (durationMs * k) / targetChunks
      let candidates = silences.filter { abs($0.midMs - ideal) <= 8000 && $0.midMs > cuts.last! + 5000 && $0.midMs < durationMs - 5000 }
      if let best = candidates.min(by: { abs($0.midMs - ideal) < abs($1.midMs - ideal) }) {
        cuts.append(best.midMs)
      }
    }
    cuts.append(durationMs)
    guard cuts.count > 2 else { return nil }

    let dir = FileManager.default.temporaryDirectory.appending(path: "scrawl-wav-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var files: [(offsetMs: Int, url: URL)] = []
    for idx in 0..<(cuts.count - 1) {
      let c0 = cuts[idx]
      let c1 = cuts[idx + 1]
      let byte0 = pcmStart + c0 * 32
      let byte1 = min(data.count, pcmStart + c1 * 32)
      let sliceLen = byte1 - byte0
      guard sliceLen > 0 else { continue }
      var header = Data()
      func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
      func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
      header.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + sliceLen))
      header.append(contentsOf: Array("WAVE".utf8))
      header.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(16000); u32(32000); u16(2); u16(16)
      header.append(contentsOf: Array("data".utf8)); u32(UInt32(sliceLen))
      var chunkData = Data(capacity: 44 + sliceLen)
      chunkData.append(header)
      chunkData.append(data[byte0..<byte1])
      let chunkUrl = dir.appending(path: "chunk-\(idx).wav")
      try chunkData.write(to: chunkUrl)
      files.append((c0, chunkUrl))
    }
    return WavChunks(dir: dir, files: files)
  }

  private static func ms(_ time: CMTime) -> Int {
    time.seconds.isFinite ? max(0, Int((time.seconds * 1000).rounded())) : 0
  }
}

enum Media {
  static func probe(_ movie: URL) async throws -> [String: Any] {
    let asset = AVURLAsset(url: movie)
    let duration = try await asset.load(.duration).seconds
    var result: [String: Any] = ["durationMs": Int(((duration.isFinite ? duration : 0) * 1000).rounded())]
    if let track = try await asset.loadTracks(withMediaType: .video).first {
      let (size, transform) = try await track.load(.naturalSize, .preferredTransform)
      let shown = size.applying(transform)
      result["width"] = Int(abs(shown.width).rounded())
      result["height"] = Int(abs(shown.height).rounded())
      result["fps"] = Double(try await track.load(.nominalFrameRate))
    }
    result["audioTracks"] = try await asset.loadTracks(withMediaType: .audio).count
    return result
  }

  /// Decodes frames as raw RGBA8 (`frame-NNNN.rgba`), either sampled every `everyMs` (nearest
  /// decodable frame, for keyframe candidates) or exactly at `atMs` (for the keyframes themselves).
  static func frames(movie: URL, outDir: URL, everyMs: Int?, atMs: [Int]?, maxEdge: Int) async throws -> [[String: Any]] {
    let asset = AVURLAsset(url: movie)
    let duration = try await asset.load(.duration).seconds
    let durationMs = Int((duration.isFinite ? duration : 0) * 1000)
    guard durationMs > 0 else { throw HelperError("Movie has no duration") }
    try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

    let isEvery = (everyMs ?? 0) > 0
    let requested: [Int] = if let everyMs, everyMs > 0 {
      Array(stride(from: 0, to: durationMs, by: everyMs))
    } else {
      (atMs ?? []).map { min(max(0, $0), max(0, durationMs - 1)) }
    }
    guard !requested.isEmpty else { return [] }
    let tolerance = isEvery
      ? CMTime(value: CMTimeValue((everyMs ?? 1) / 2), timescale: 1000)
      : CMTime(value: 80, timescale: 1000)
    let workerCount = min(6, requested.count)
    let chunkSize = (requested.count + workerCount - 1) / workerCount

    let indexed = try await withThrowingTaskGroup(of: [(Int, [String: Any])].self) { group in
      for worker in 0..<workerCount {
        let start = worker * chunkSize
        let end = min(start + chunkSize, requested.count)
        if start >= end { continue }
        let sliceReq = Array(requested[start..<end])
        group.addTask {
          let generator = AVAssetImageGenerator(asset: asset)
          generator.appliesPreferredTrackTransform = true
          if maxEdge > 0 { generator.maximumSize = CGSize(width: maxEdge, height: maxEdge) }
          generator.requestedTimeToleranceBefore = tolerance
          generator.requestedTimeToleranceAfter = tolerance
          let sliceTimes = sliceReq.map { CMTime(value: CMTimeValue($0), timescale: 1000) }
          var local: [(Int, [String: Any])] = []
          var offset = 0
          for await result in generator.images(for: sliceTimes) {
            let curIndex = start + offset
            offset += 1
            guard let image = try? result.image else { continue }
            let reqTime = result.requestedTime.seconds.isFinite
              ? Int((result.requestedTime.seconds * 1000).rounded())
              : sliceReq[min(offset - 1, sliceReq.count - 1)]
            let actual = (try? result.actualTime).map { Int(($0.seconds * 1000).rounded()) } ?? reqTime
            let file = outDir.appending(path: String(format: "frame-%04d.rgba", curIndex))
            try writeRGBA(image, to: file)
            local.append((curIndex, ["t": reqTime, "actual": actual, "width": image.width, "height": image.height, "file": file.path]))
          }
          return local
        }
      }
      var pairs: [(Int, [String: Any])] = []
      for try await batch in group {
        pairs.append(contentsOf: batch)
      }
      return pairs
    }
    return indexed.sorted { $0.0 < $1.0 }.map(\.1)
  }

  private static let srgbSpace = CGColorSpace(name: CGColorSpace.sRGB)!

  private static func writeRGBA(_ image: CGImage, to url: URL) throws {
    let width = image.width, height = image.height
    var pixels = Data(count: width * height * 4)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
      guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                    space: srgbSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
      else { return false }
      context.interpolationQuality = .high
      context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
      return true
    }
    guard drawn else { throw HelperError("Could not convert frame") }
    try pixels.write(to: url)
  }

  /// Re-encodes a movie's video to HEVC with its long edge at most `maxEdge`, copying every audio
  /// track as it is. Each sample keeps its presentation time, so the timeline (and every time in
  /// `scrawl.json`) still lines up. Fails rather than writing a movie of a different length.
  static func shrink(movie: URL, out: URL, maxEdge: Int, bitRate: Int) async throws -> [String: Any] {
    let started = Date()
    let asset = AVURLAsset(url: movie)
    guard let videoTrack = try await asset.loadTracks(withMediaType: .video).first else { throw HelperError("Movie has no video") }
    let audioTracks = try await asset.loadTracks(withMediaType: .audio)
    let (natural, transform) = try await videoTrack.load(.naturalSize, .preferredTransform)
    let duration = try await asset.load(.duration)
    let (w, h) = (abs(natural.width), abs(natural.height))
    let scale = min(1, Double(maxEdge) / Double(max(w, h)))
    func even(_ v: Double) -> Int { max(2, Int((v / 2).rounded()) * 2) }
    let (outW, outH) = (even(w * scale), even(h * scale))

    try? FileManager.default.removeItem(at: out)
    let reader = try AVAssetReader(asset: asset)
    let writer = try AVAssetWriter(outputURL: out, fileType: .mov)
    writer.movieTimeScale = duration.timescale
    writer.metadata = (try? await asset.load(.metadata)) ?? []

    let videoOut = AVAssetReaderTrackOutput(track: videoTrack, outputSettings: [
      kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
      kCVPixelBufferWidthKey as String: outW,
      kCVPixelBufferHeightKey as String: outH,
    ])
    videoOut.alwaysCopiesSampleData = false
    reader.add(videoOut)
    let videoIn = AVAssetWriterInput(mediaType: .video, outputSettings: [
      AVVideoCodecKey: AVVideoCodecType.hevc,
      AVVideoWidthKey: outW,
      AVVideoHeightKey: outH,
      AVVideoCompressionPropertiesKey: [
        AVVideoAverageBitRateKey: bitRate,
        AVVideoMaxKeyFrameIntervalDurationKey: 2,
        AVVideoAllowFrameReorderingKey: true,
      ],
    ])
    videoIn.transform = transform
    videoIn.expectsMediaDataInRealTime = false
    videoIn.mediaTimeScale = try await videoTrack.load(.naturalTimeScale)
    writer.add(videoIn)

    var pairs: [(AVAssetReaderOutput, AVAssetWriterInput)] = [(videoOut, videoIn)]
    for track in audioTracks {
      let output = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
      reader.add(output)
      let hint = try await track.load(.formatDescriptions).first
      let input = AVAssetWriterInput(mediaType: .audio, outputSettings: nil, sourceFormatHint: hint)
      input.expectsMediaDataInRealTime = false
      writer.add(input)
      pairs.append((output, input))
    }

    guard reader.startReading() else { throw HelperError(reader.error?.localizedDescription ?? "Could not read the movie") }
    guard writer.startWriting() else { throw HelperError(writer.error?.localizedDescription ?? "Could not write the movie") }
    writer.startSession(atSourceTime: .zero)

    await withTaskGroup(of: Void.self) { group in
      for (index, (output, input)) in pairs.enumerated() {
        nonisolated(unsafe) let output = output
        nonisolated(unsafe) let input = input
        nonisolated(unsafe) let reader = reader
        group.addTask {
          await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            let queue = DispatchQueue(label: "shrink.\(index)", qos: .utility)
            input.requestMediaDataWhenReady(on: queue) {
              while input.isReadyForMoreMediaData {
                guard let buffer = output.copyNextSampleBuffer() else {
                  input.markAsFinished()
                  done.resume()
                  return
                }
                if !input.append(buffer) {
                  reader.cancelReading()
                  input.markAsFinished()
                  done.resume()
                  return
                }
              }
            }
          }
        }
      }
    }
    if reader.status == .failed { writer.cancelWriting(); throw HelperError(reader.error?.localizedDescription ?? "Reading the movie failed") }
    await writer.finishWriting()
    guard writer.status == .completed else { throw HelperError(writer.error?.localizedDescription ?? "Writing the movie failed") }

    let written = try await AVURLAsset(url: out).load(.duration)
    let drift = abs(written.seconds - duration.seconds)
    guard drift < 0.1 else {
      try? FileManager.default.removeItem(at: out)
      throw HelperError(String(format: "The shrunk movie is %.2f s off the original", drift))
    }
    func size(_ url: URL) -> Int { (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0 }
    return [
      "before": size(movie),
      "after": size(out),
      "width": outW,
      "height": outH,
      "durationMs": Int((written.seconds * 1000).rounded()),
      "ms": Int(Date().timeIntervalSince(started) * 1000),
    ]
  }

  /// Every audio track mixed to 16 kHz mono 16-bit PCM, padded so sample 0 is the movie's start.
  static func exportAudio(movie: URL, wav: URL) async throws -> [String: Any] {
    let asset = AVURLAsset(url: movie)
    let tracks = try await asset.loadTracks(withMediaType: .audio)
    guard !tracks.isEmpty else { throw HelperError("Movie has no audio") }
    let reader = try AVAssetReader(asset: asset)
    let output = AVAssetReaderAudioMixOutput(audioTracks: tracks, audioSettings: [
      AVFormatIDKey: kAudioFormatLinearPCM,
      AVSampleRateKey: 16000,
      AVNumberOfChannelsKey: 1,
      AVLinearPCMBitDepthKey: 16,
      AVLinearPCMIsFloatKey: false,
      AVLinearPCMIsBigEndianKey: false,
      AVLinearPCMIsNonInterleaved: false,
    ])
    reader.add(output)
    guard reader.startReading() else { throw HelperError(reader.error?.localizedDescription ?? "Could not read audio") }
    var pcm = Data()
    var first = true
    while let buffer = output.copyNextSampleBuffer() {
      if first {
        first = false
        let start = CMSampleBufferGetPresentationTimeStamp(buffer).seconds
        if start.isFinite, start > 0 { pcm.append(Data(count: Int(start * 16000) * 2)) }
      }
      guard let block = CMSampleBufferGetDataBuffer(buffer) else { continue }
      let length = CMBlockBufferGetDataLength(block)
      var chunk = Data(count: length)
      chunk.withUnsafeMutableBytes { _ = CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: length, destination: $0.baseAddress!) }
      pcm.append(chunk)
    }
    if reader.status == .failed { throw HelperError(reader.error?.localizedDescription ?? "Audio export failed") }
    var header = Data()
    func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
    func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
    header.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + pcm.count))
    header.append(contentsOf: Array("WAVE".utf8))
    header.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(16000); u32(32000); u16(2); u16(16)
    header.append(contentsOf: Array("data".utf8)); u32(UInt32(pcm.count))
    try (header + pcm).write(to: wav)
    return ["path": wav.path, "samples": pcm.count / 2, "durationMs": pcm.count / 32]
  }
}
