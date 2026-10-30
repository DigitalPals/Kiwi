import AVFoundation
import Foundation

// Local audio/model tests use synthetic speech, never a user's recording.
@main
struct SpeechActivityTests {
    struct Classification {
        let start: Double, end: Double, speech: Double, wind: Double
    }

    final class Results {
        private let lock = NSLock()
        private var values: [Classification] = []
        private var failed = false
        func receive(_ event: StreamSpeechDetector.Event) {
            lock.lock(); defer { lock.unlock() }
            switch event {
            case .classification(let start, let end, let speech, let wind):
                values.append(Classification(start: start, end: end, speech: speech, wind: wind))
            case .unavailable: failed = true
            }
        }
        func snapshot() -> ([Classification], Bool) {
            lock.lock(); defer { lock.unlock() }
            return (values, failed)
        }
    }

    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw RamblerError.invalid("FAIL: \(message)") }
        print("PASS: \(message)")
    }

    static func little<T: FixedWidthInteger>(_ number: T) -> Data {
        var value = number.littleEndian
        return withUnsafeBytes(of: &value) { Data($0) }
    }

    static func wav(_ samples: [Float]) -> Data {
        let pcm = samples.reduce(into: Data()) { $0 += little(Int16(max(-32_767, min(32_767, Double($1) * 32_767)))) }
        var header = Data("RIFF".utf8) + little(UInt32(pcm.count + 36)) + Data("WAVEfmt ".utf8)
        header += little(UInt32(16)) + little(UInt16(1)) + little(UInt16(1))
        header += little(UInt32(16_000)) + little(UInt32(32_000))
        header += little(UInt16(2)) + little(UInt16(16)) + Data("data".utf8)
        header += little(UInt32(0)) // Growing recorder has no final data length yet.
        return header + pcm
    }

    static func classify(_ samples: [Float], directory: URL, name: String, growing: Bool = false) throws -> [Classification] {
        let url = directory.appendingPathComponent(name + ".wav")
        let bytes = wav(samples)
        try (growing ? bytes.prefix(12) : bytes).write(to: url)
        let results = Results()
        let detector = StreamSpeechDetector(url: url, receive: results.receive)
        defer { detector.stop() }
        if growing {
            Thread.sleep(forTimeInterval: 0.2)
            let output = try FileHandle(forWritingTo: url)
            try output.seekToEnd()
            for offset in stride(from: 12, to: bytes.count, by: 3200) {
                try output.write(contentsOf: bytes[offset..<min(bytes.count, offset + 3200)])
                Thread.sleep(forTimeInterval: 0.01)
            }
            try output.close()
        }
        let deadline = Date().addingTimeInterval(20)
        let duration = Double(samples.count) / 16_000
        while Date() < deadline {
            let (values, failed) = results.snapshot()
            if failed { throw RamblerError.invalid("The local sound classifier failed for \(name).") }
            if (values.last?.end ?? 0) >= duration - 0.6 { return values }
            Thread.sleep(forTimeInterval: 0.05)
        }
        throw RamblerError.invalid("The local sound classifier stalled for \(name).")
    }

    static func endpointChecks() throws {
        var endpoint = SpeechEndpoint()
        endpoint.classify(start: 0, end: 0.5, speech: 0.2, wind: 0.9)
        try check(!endpoint.heardSpeech && !endpoint.shouldFinish(time: 10), "Wind alone does not start or submit dictation")
        endpoint.classify(start: 0.5, end: 1, speech: 0.8, wind: 0.1)
        endpoint.classify(start: 1, end: 1.5, speech: 0.4, wind: 0.1)
        try check(endpoint.lastSpeech == 1.5, "Hysteresis retains softer speech after speech starts")
        try check(!endpoint.shouldFinish(time: 2.5), "Classifier latency cannot masquerade as silence")
        endpoint.classify(start: 1.5, end: 2, speech: 0.01, wind: 0.9)
        try check(!endpoint.shouldFinish(time: 2.1), "Short pauses do not cut off speech")
        endpoint.classify(start: 2, end: 2.5, speech: 0.01, wind: 0.9)
        endpoint.meter(power: -25, time: 2.5)
        try check(endpoint.shouldFinish(time: 2.5), "Finish after one second of classified silence despite loud wind")
        endpoint.classify(start: 0, end: 1, speech: 1, wind: 0)
        try check(endpoint.classifiedThrough == 2.5 && !endpoint.shouldFinish(time: .nan), "Ignore stale and invalid detector timestamps")
        var fallback = SpeechEndpoint()
        for tick in 0...15 { fallback.meter(power: -65, time: Double(tick) / 10) }
        fallback.meter(power: -42, time: 1.6)
        fallback.meter(power: -42, time: 1.7)
        try check(!fallback.heardSpeech, "Energy fallback rejects a brief impulse")
        fallback.meter(power: -42, time: 1.8)
        fallback.meter(power: -65, time: 1.9)
        try check(fallback.heardSpeech && fallback.shouldFinish(time: 2.81), "Unavailable classifier retains a bounded energy fallback")
        var noise = SpeechEndpoint()
        for tick in 1...80 {
            let t = Double(tick) / 10
            noise.classify(start: max(0, t - 0.5), end: t, speech: 0.03, wind: 0.8)
            noise.meter(power: -30, time: t)
        }
        try check(!noise.heardSpeech, "A loud steady noise floor does not override the speech classifier")
        var stalled = SpeechEndpoint()
        for tick in 0...5 { stalled.meter(power: -65, time: Double(tick) / 10) }
        stalled.classify(start: 0.5, end: 1, speech: 0.8, wind: 0)
        var cutOff = false
        for tick in 10...60 {
            let time = Double(tick) / 10
            stalled.meter(power: -40, time: time)
            cutOff = cutOff || stalled.shouldFinish(time: time)
        }
        try check(!cutOff && stalled.lastSpeech >= 5.9, "A classifier stall does not cut off ongoing speech or learn it as noise")
    }

    static func parserChecks() throws {
        let data = wav([Float](repeating: 0, count: 8000))
        try check(try StreamSpeechDetector.pcmPayload(data) == 44, "Read PCM before the recorder finalizes the WAV data length")
        try check(try StreamSpeechDetector.pcmPayload(data.prefix(30)) == nil, "Wait for an incomplete live WAV header")
        var stereo = data
        stereo[22] = 2
        do {
            _ = try StreamSpeechDetector.pcmPayload(stereo)
            throw RamblerError.invalid("FAIL: reject unsupported live PCM")
        } catch RamblerError.invalid(let message) {
            try check(!message.hasPrefix("FAIL:"), "Reject unsupported live PCM rather than misreading it")
        }
    }

    static func filterChecks() throws {
        func gain(frequency: Double) -> Double {
            let format = AVAudioFormat(standardFormatWithSampleRate: 16_000, channels: 1)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 16_000)!
            buffer.frameLength = 16_000
            let samples = buffer.floatChannelData![0]
            for index in 0..<16_000 { samples[index] = Float(0.2 * sin(2 * .pi * frequency * Double(index) / 16_000)) }
            DictationAudio.highPass(buffer)
            let energy = (1600..<16_000).reduce(0.0) { $0 + Double(samples[$1] * samples[$1]) }
            return sqrt(energy / 14_400) / (0.2 / sqrt(2))
        }
        try check(gain(frequency: 30) < 0.1, "Reduce 30 Hz wind rumble by more than 20 dB")
        try check(gain(frequency: 1000) > 0.99, "Preserve the 1 kHz speech band")
    }

    static func firstFinish(_ values: [Classification], samples: [Float]) -> Double? {
        var endpoint = SpeechEndpoint()
        var index = 0
        for tick in 1...Int(Double(samples.count) / 1600) {
            let time = Double(tick) / 10
            // Include modest file/model latency in the virtual recorder clock.
            while index < values.count && values[index].end + 0.2 <= time {
                let value = values[index]
                endpoint.classify(start: value.start, end: value.end, speech: value.speech, wind: value.wind)
                index += 1
            }
            let window = samples[(tick - 1) * 1600..<tick * 1600]
            let rms = sqrt(window.reduce(0.0) { $0 + Double($1 * $1) } / 1600)
            endpoint.meter(power: 20 * log10(max(0.00000001, rms)), time: time)
            if endpoint.shouldFinish(time: time) { return time }
        }
        return nil
    }

    static func main() async {
        do {
            guard CommandLine.arguments.count >= 2 else { throw RamblerError.invalid("Usage: speech-tests SAMPLE.wav [CALLER.json]") }
            try endpointChecks()
            try parserChecks()
            try filterChecks()
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("kiwi-speech-tests-\(UUID())")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            let input = try AVAudioFile(forReading: URL(fileURLWithPath: CommandLine.arguments[1]), commonFormat: .pcmFormatFloat32, interleaved: false)
            try check(input.processingFormat.sampleRate == 16_000 && input.processingFormat.channelCount == 1, "Use a mono 16 kHz synthetic speech fixture")
            let source = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: AVAudioFrameCount(input.length))!
            try input.read(into: source)
            let original = Array(UnsafeBufferPointer(start: source.floatChannelData![0], count: Int(source.frameLength)))
            let silence = [Float](repeating: 0, count: 4 * 16_000)
            var random: UInt64 = 42
            var low = 0.0
            let wind: [Float] = (0..<(5 * 16_000)).map { index in
                random = random &* 6364136223846793005 &+ 1
                let white = Double(random >> 32) / Double(UInt32.max) * 2 - 1
                low = 0.98 * low + 0.02 * white
                return Float(0.5 * low + 0.06 * sin(2 * .pi * 45 * Double(index) / 16_000))
            }
            let silentResults = try classify(silence, directory: directory, name: "silence")
            try check(silentResults.allSatisfy { $0.speech < 0.5 }, "The actual Apple model rejects silence")
            let windResults = try classify(wind, directory: directory, name: "wind")
            try check(windResults.allSatisfy { $0.speech < 0.5 || $0.speech <= $0.wind }, "The actual Apple model rejects synthetic wind-like noise")
            // Quiet speech is deliberately below the old -38 dB volume gate.
            let peak = original.map { abs($0) }.max()!
            let quiet = original.map { $0 * 0.03 / peak }
            let padded = [Float](repeating: 0, count: 2 * 16_000) + quiet + silence
            let values = try classify(padded, directory: directory, name: "quiet-growing", growing: true)
            try check(values.contains { $0.speech >= 0.5 }, "The actual Apple model detects quiet speech from a growing recorder file")
            var endpoint = SpeechEndpoint()
            for value in values { endpoint.classify(start: value.start, end: value.end, speech: value.speech, wind: value.wind) }
            try check(endpoint.heardSpeech && endpoint.shouldFinish(time: Double(padded.count) / 16_000), "A speech recording reaches its endpoint after the trailing pause")
            let speechEnd = 2 + Double(original.count) / 16_000
            try check(endpoint.lastSpeech >= speechEnd - 0.7 && endpoint.lastSpeech < speechEnd + 1, "Preserve the final word without treating the entire trailing pause as speech")
            let finished = firstFinish(values, samples: padded) ?? .infinity
            try check(finished >= speechEnd && finished < speechEnd + 1.9, "Replay the whole utterance without an early cutoff and finish promptly after speech")
            let noisy = (padded.indices).map { index in padded[index] / 0.03 * 0.4 + wind[index % wind.count] * 0.4 }
            let noisyValues = try classify(noisy, directory: directory, name: "speech-with-wind")
            let noisyFinished = firstFinish(noisyValues, samples: noisy) ?? .infinity
            try check(noisyFinished >= speechEnd && noisyFinished < speechEnd + 1.9, "Continuous wind-like noise after speech does not prolong the recording")
            let url = directory.appendingPathComponent("quiet-growing.wav")
            // Finalize the fixture for the ordinary AVAudioFile reader.
            var finalized = try Data(contentsOf: url)
            finalized.replaceSubrange(40..<44, with: little(UInt32(finalized.count - 44)))
            try finalized.write(to: url)
            let encoded = try DictationAudio.encode(file: url)
            try check(encoded.audio.starts(with: Data("OggS".utf8)) && abs(encoded.originalSeconds - encoded.submittedSeconds) < 1 / 16_000,
                "Preserve the entire recording, including the opening and pauses, in valid Ogg Opus")
            if let path = ProcessInfo.processInfo.environment["KIWI_SPEECH_TEST_ARTIFACTS"] {
                // Optional black-box codec verification uses only this synthetic
                // fixture, with an independent decoder outside the Swift client.
                let artifacts = URL(fileURLWithPath: path)
                try FileManager.default.createDirectory(at: artifacts, withIntermediateDirectories: true)
                try finalized.write(to: artifacts.appendingPathComponent("complete-recording.wav"))
                try encoded.audio.write(to: artifacts.appendingPathComponent("complete-recording.ogg"))
            }
            print(String(format: "Audio: %.2fs source, %.2fs submitted; last speech %.2fs", encoded.originalSeconds, encoded.submittedSeconds, endpoint.lastSpeech))
            if CommandLine.arguments.count > 2 {
                let caller = try JSONDecoder().decode(CallerConfiguration.self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2])))
                let result = try await RamblerClient(caller: caller).transcribe(encoded.audio)
                let text = result.text.lowercased()
                try check(["meet", "wednesday", "budget", "report"].allSatisfy { text.contains($0) } && !text.contains("tuesday"),
                    "Live Rambler preserves the opening, spoken correction and final words from the complete recording")
            }
            print("PASS: speech activity tests")
        } catch {
            print(error.localizedDescription)
            exit(1)
        }
    }
}
