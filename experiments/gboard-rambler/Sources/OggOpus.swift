import AVFoundation
import Foundation

// Apple's native Opus encoder supplies packets. Ogg is only a container; no
// Android library or model is used. Granule positions always use the 48 kHz clock.
enum OggOpus {
    static func encode(file: URL) throws -> Data {
        let input = try AVAudioFile(forReading: file)
        let duration = Double(input.length) / input.processingFormat.sampleRate
        guard duration > 0, duration <= 120 else {
            throw RamblerError.invalid("Record between one moment and two minutes of speech.")
        }
        let source = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: AVAudioFrameCount(input.length))!
        try input.read(into: source)
        let pcmFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!
        guard let resampler = AVAudioConverter(from: source.format, to: pcmFormat) else {
            throw RamblerError.invalid("This recording's audio format is unsupported.")
        }
        let pcm = AVAudioPCMBuffer(pcmFormat: pcmFormat, frameCapacity: AVAudioFrameCount(ceil(duration * 16_000)) + 1024)!
        var supplied = false
        var error: NSError?
        let resampleStatus = resampler.convert(to: pcm, error: &error) { _, state in
            if supplied { state.pointee = .endOfStream; return nil }
            supplied = true
            state.pointee = .haveData
            return source
        }
        guard resampleStatus != .error, pcm.frameLength > 0 else {
            throw RamblerError.invalid("Could not convert the recording to mono audio.")
        }
        let format = AVAudioFormat(settings: [AVFormatIDKey: kAudioFormatOpus,
            AVSampleRateKey: 16_000.0, AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 24_000])!
        guard let converter = AVAudioConverter(from: pcmFormat, to: format) else {
            throw RamblerError.invalid("The native Opus encoder is unavailable on this device.")
        }
        let capacity = AVAudioPacketCount(pcm.frameLength / 320 + 16)
        let output = AVAudioCompressedBuffer(format: format, packetCapacity: capacity, maximumPacketSize: converter.maximumOutputPacketSize)
        supplied = false
        error = nil
        let status = converter.convert(to: output, error: &error) { _, state in
            if supplied { state.pointee = .endOfStream; return nil }
            supplied = true
            state.pointee = .haveData
            return pcm
        }
        guard status != .error, output.packetCount > 0,
              let descriptions = output.packetDescriptions else {
            throw RamblerError.invalid("Could not encode the recording as Opus.")
        }
        let leading = UInt64(converter.primeInfo.leadingFrames) * 3
        let trailing = UInt64(converter.primeInfo.trailingFrames) * 3
        guard leading <= UInt16.max else { throw RamblerError.invalid("Unexpected Opus encoder delay.") }
        var head = Data("OpusHead".utf8) + Data([1, 1])
        head += little(UInt16(leading)) + little(UInt32(16_000)) + little(UInt16(0)) + Data([0])
        let vendor = Data("Kiwi Rambler PoC".utf8)
        let tags = Data("OpusTags".utf8) + little(UInt32(vendor.count)) + vendor + little(UInt32(0))
        let serial = UInt32.random(in: 1 ... UInt32.max)
        var out = try page(head, sequence: 0, serial: serial, flags: 2, granule: 0)
        out += try page(tags, sequence: 1, serial: serial, flags: 0, granule: 0)
        var granule: UInt64 = 0
        for index in 0 ..< Int(output.packetCount) {
            let description = descriptions[index]
            let offset = Int(description.mStartOffset)
            let count = Int(description.mDataByteSize)
            guard offset >= 0, count > 0, offset + count <= Int(output.byteLength) else {
                throw RamblerError.invalid("Invalid native Opus packet.")
            }
            let last = index == Int(output.packetCount) - 1
            let packet = Data(bytes: output.data.advanced(by: offset), count: count)
            granule += try packetSamples(packet)
            out += try page(packet, sequence: UInt32(index + 2), serial: serial,
                            flags: last ? 4 : 0, granule: last ? granule - min(granule, trailing) : granule)
        }
        return out
    }

    // RFC 6716, section 3: Core Audio may leave both frames-per-packet fields
    // zero for VBR Opus. The packet TOC still gives the exact duration.
    private static func packetSamples(_ packet: Data) throws -> UInt64 {
        let bytes = [UInt8](packet)
        let toc = bytes[0]
        let shift = Int((toc >> 3) & 3)
        let samples: Int
        if toc & 128 != 0 { samples = 120 << shift }
        else if toc & 96 == 96 { samples = toc & 8 == 0 ? 480 : 960 }
        else { samples = shift == 3 ? 2880 : 480 << shift }
        let frames: Int
        switch toc & 3 {
        case 0: frames = 1
        case 1, 2: frames = 2
        default:
            guard bytes.count >= 2 else { throw RamblerError.invalid("Truncated Opus packet.") }
            frames = Int(bytes[1] & 63)
        }
        guard frames > 0, samples * frames <= 5760 else { throw RamblerError.invalid("Invalid Opus packet duration.") }
        return UInt64(samples * frames)
    }

    private static func little<T: FixedWidthInteger>(_ value: T) -> Data {
        var value = value.littleEndian
        return withUnsafeBytes(of: &value) { Data($0) }
    }

    private static func page(_ packet: Data, sequence: UInt32, serial: UInt32, flags: UInt8, granule: UInt64) throws -> Data {
        let fullSegments = packet.count / 255
        guard fullSegments < 255 else { throw RamblerError.invalid("Opus packet exceeds the Ogg page limit.") }
        let lacing = Data(repeating: 255, count: fullSegments) + Data([UInt8(packet.count % 255)])
        var page = Data("OggS".utf8) + Data([0, flags]) + little(granule) + little(serial)
            + little(sequence) + little(UInt32(0)) + Data([UInt8(lacing.count)]) + lacing + packet
        var crc: UInt32 = 0
        for byte in page {
            crc ^= UInt32(byte) << 24
            for _ in 0 ..< 8 { crc = (crc << 1) ^ (crc & 0x80000000 == 0 ? 0 : 0x04c11db7) }
        }
        page.replaceSubrange(22 ..< 26, with: little(crc))
        return page
    }
}
