import AVFoundation
import SoundAnalysis

// Recording stays with the already-tested AVAudioRecorder. Read its growing
// mono PCM file on a serial queue, without opening another microphone/audio unit.
final class StreamSpeechDetector: NSObject, SNResultsObserving {
    enum Event {
        case classification(start: Double, end: Double, speech: Double, wind: Double)
        case unavailable
    }

    private let queue = DispatchQueue(label: "com.kiwi.rambler.speech-activity", qos: .userInitiated)
    private let url: URL
    private let receive: (Event) -> Void
    private var timer: DispatchSourceTimer?
    private var file: FileHandle?
    private var analyzer: SNAudioStreamAnalyzer?
    private var format: AVAudioFormat?
    private var frame: AVAudioFramePosition = 0
    private var payload: UInt64?
    private var stopped = false

    init(url: URL, receive: @escaping (Event) -> Void) {
        self.url = url
        self.receive = receive
        super.init()
        queue.async { self.start() }
    }

    private func start() {
        do {
            let format = AVAudioFormat(standardFormatWithSampleRate: 16_000, channels: 1)!
            let analyzer = SNAudioStreamAnalyzer(format: format)
            let request = try SNClassifySoundRequest(classifierIdentifier: .version1)
            // Apple's built-in classifier supports windows from 0.5 seconds.
            // Its default three-second window is too slow for dictation pauses.
            request.windowDuration = CMTime(seconds: 0.5, preferredTimescale: 16_000)
            request.overlapFactor = 0.8
            try analyzer.add(request, withObserver: self)
            self.format = format
            self.analyzer = analyzer
            let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now(), repeating: .milliseconds(100))
            timer.setEventHandler { [weak self] in self?.readAvailable() }
            self.timer = timer
            timer.resume()
        } catch { unavailable() }
    }

    private func unavailable() {
        guard !stopped else { return }
        receive(.unavailable)
        tearDown()
    }

    private func readAvailable() {
        guard !stopped, let format, let analyzer else { return }
        do {
            if file == nil { file = try FileHandle(forReadingFrom: url) }
            guard let file else { return }
            if payload == nil {
                try file.seek(toOffset: 0)
                guard let header = try file.read(upToCount: 65_536) else { return }
                payload = try Self.pcmPayload(header)
                guard let payload else { return } // Header is still being written.
                try file.seek(toOffset: payload)
            }
            guard var bytes = try file.read(upToCount: 32_000), !bytes.isEmpty else { return }
            if bytes.count % 2 != 0 {
                bytes.removeLast()
                try file.seek(toOffset: try file.offset() - 1)
            }
            let count = bytes.count / 2
            guard count > 0, let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(count)),
                  let samples = buffer.floatChannelData?[0] else { return }
            let raw = [UInt8](bytes)
            for index in 0 ..< count {
                let value = UInt16(raw[2 * index]) | UInt16(raw[2 * index + 1]) << 8
                samples[index] = Float(Int16(bitPattern: value)) / 32_768
            }
            buffer.frameLength = AVAudioFrameCount(count)
            analyzer.analyze(buffer, atAudioFramePosition: frame)
            frame += AVAudioFramePosition(count)
        } catch { unavailable() }
    }

    // RIFF's data length may remain zero until AVAudioRecorder closes the file.
    // Use the actual available bytes, while validating the PCM format strictly.
    static func pcmPayload(_ data: Data) throws -> UInt64? {
        let bytes = [UInt8](data)
        guard bytes.count >= 12 else { return nil }
        guard String(bytes: bytes[0..<4], encoding: .ascii) == "RIFF",
              String(bytes: bytes[8..<12], encoding: .ascii) == "WAVE" else {
            throw RamblerError.invalid("Unsupported live recording container.")
        }
        func number(_ offset: Int, _ count: Int) -> UInt32 {
            (0..<count).reduce(0) { $0 | UInt32(bytes[offset + $1]) << ($1 * 8) }
        }
        var offset = 12
        var valid = false
        while offset + 8 <= bytes.count {
            let name = String(bytes: bytes[offset..<offset + 4], encoding: .ascii)
            let size = Int(number(offset + 4, 4))
            let start = offset + 8
            if name == "data" {
                guard valid else { throw RamblerError.invalid("Missing recording PCM format.") }
                return UInt64(start)
            }
            guard size <= 65_536, start + size <= bytes.count else { return nil }
            if name == "fmt " {
                guard size >= 16 else { throw RamblerError.invalid("Invalid recording PCM format.") }
                let code = number(start, 2)
                let pcm = code == 1 || (code == 0xfffe && size >= 40 && number(start + 24, 2) == 1)
                guard pcm, number(start + 2, 2) == 1, number(start + 4, 4) == 16_000,
                      number(start + 12, 2) == 2, number(start + 14, 2) == 16 else {
                    throw RamblerError.invalid("Unexpected recording PCM format.")
                }
                valid = true
            }
            offset = start + size + size % 2
        }
        return nil
    }

    func request(_ request: SNRequest, didProduce result: SNResult) {
        guard let result = result as? SNClassificationResult else { return }
        let speech = result.classification(forIdentifier: "speech")?.confidence ?? 0
        let wind = max(result.classification(forIdentifier: "wind")?.confidence ?? 0,
                       result.classification(forIdentifier: "wind_noise_microphone")?.confidence ?? 0)
        receive(.classification(start: CMTimeGetSeconds(result.timeRange.start),
            end: CMTimeGetSeconds(CMTimeRangeGetEnd(result.timeRange)), speech: speech, wind: wind))
    }

    func request(_ request: SNRequest, didFailWithError error: Error) {
        queue.async { self.unavailable() }
    }

    func requestDidComplete(_ request: SNRequest) {}

    func stop() { queue.async { self.tearDown() } }

    private func tearDown() {
        stopped = true
        timer?.cancel(); timer = nil
        analyzer?.removeAllRequests(); analyzer = nil
        try? file?.close(); file = nil
    }
}

// Recorded time, rather than wall time, keeps pauses stable across app switches.
struct SpeechEndpoint {
    static let pause = 1.0
    private(set) var firstSpeech: Double?
    private(set) var lastSpeech = 0.0
    private(set) var classifiedThrough = 0.0
    private(set) var confidence = 0.0
    private(set) var noiseFloor = -60.0
    private var powers: [Double] = []
    private var candidate: Double?

    var heardSpeech: Bool { firstSpeech != nil }

    mutating func classify(start: Double, end: Double, speech: Double, wind: Double) {
        guard start.isFinite, end.isFinite, speech.isFinite, wind.isFinite,
              start >= 0, end > start, end >= classifiedThrough else { return }
        classifiedThrough = end
        confidence = speech
        let threshold = heardSpeech ? 0.35 : 0.5
        if speech >= threshold && speech > wind {
            firstSpeech = min(firstSpeech ?? start, start)
            lastSpeech = max(lastSpeech, end)
        }
    }

    mutating func meter(power: Double, time: Double) {
        guard time.isFinite, time >= 0 else { return }
        let power = power.isFinite ? max(-160, min(0, power)) : -160
        powers.append(power)
        if powers.count > 30 { powers.removeFirst() }
        let sorted = powers.sorted()
        let estimate = sorted[max(0, sorted.count / 5)]
        // Do not learn an ongoing utterance as the ambient noise floor. This
        // also keeps the fallback useful if classification stalls mid-sentence.
        noiseFloor = heardSpeech ? min(noiseFloor, estimate) : estimate
        // Fall back if the model is unavailable or its results have stalled.
        // A level above a fixed threshold alone must not extend recording.
        guard (classifiedThrough == 0 && time >= 1.5) || (classifiedThrough > 0 && time - classifiedThrough > 1.5) else {
            candidate = nil
            return
        }
        let threshold = max(-45, noiseFloor + 10)
        if power > threshold {
            if candidate == nil { candidate = time }
            if time - (candidate ?? time) >= 0.15 {
                firstSpeech = firstSpeech ?? max(0, (candidate ?? time) - 0.15)
                lastSpeech = time
            }
        } else { candidate = nil }
    }

    func shouldFinish(time: Double) -> Bool {
        guard time.isFinite, heardSpeech else { return false }
        if candidate != nil { return false }
        // Wait for analyzed silence, rather than treating classifier/file-write
        // latency as a pause in speech. A stalled model uses the energy fallback.
        let through = classifiedThrough > 0 && time - classifiedThrough <= 1.5
            ? min(time, classifiedThrough) : time
        return through - lastSpeech >= Self.pause
    }

}

enum DictationAudio {
    struct Encoded {
        let audio: Data
        let originalSeconds: Double
        let submittedSeconds: Double
    }

    static func encode(file url: URL) throws -> Encoded {
        let input = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        let rate = input.processingFormat.sampleRate
        let duration = Double(input.length) / rate
        // Classification decides when to stop recording, never which recorded
        // words to send. A quiet opening may precede the first confident result
        // by several seconds. Always encode from frame zero, including pauses.
        let frames = AVAudioFrameCount(min(input.length, AVAudioFramePosition(120 * rate)))
        guard frames > 0, let pcm = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: frames) else {
            throw RamblerError.invalid("The recording contains no audio.")
        }
        input.framePosition = 0
        try input.read(into: pcm, frameCount: frames)
        highPass(pcm)
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("kiwi-speech-\(UUID()).caf")
        defer { try? FileManager.default.removeItem(at: temporary) }
        do {
            let output = try AVAudioFile(forWriting: temporary, settings: pcm.format.settings)
            try output.write(from: pcm)
        }
        return Encoded(audio: try OggOpus.encode(file: temporary), originalSeconds: duration,
            submittedSeconds: Double(pcm.frameLength) / rate)
    }

    static func highPass(_ pcm: AVAudioPCMBuffer) {
        // A gentle second-order high-pass reduces low-frequency wind/handling
        // rumble. It is not a general background-speech cancellation system.
        let k = tan(Double.pi * 100 / pcm.format.sampleRate)
        let norm = 1 / (1 + sqrt(2) * k + k * k)
        let b0 = norm, b1 = -2 * norm, b2 = norm
        let a1 = 2 * (k * k - 1) * norm, a2 = (1 - sqrt(2) * k + k * k) * norm
        for channel in 0..<Int(pcm.format.channelCount) {
            guard let samples = pcm.floatChannelData?[channel] else { continue }
            var x1 = 0.0, x2 = 0.0, y1 = 0.0, y2 = 0.0
            for frame in 0..<Int(pcm.frameLength) {
                let x = Double(samples[frame])
                let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
                samples[frame] = Float(min(1, max(-1, y)))
                x2 = x1; x1 = x; y2 = y1; y1 = y
            }
        }
    }
}
