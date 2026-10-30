import Foundation

enum RamblerError: LocalizedError {
    case invalid(String)
    var errorDescription: String? {
        switch self { case .invalid(let message): return message }
    }
}

// A small, bounded protobuf wire reader. Unknown fields are skipped, never guessed
// to be nested messages or printable strings.
enum Proto {
    struct Field {
        let number: Int
        let integer: UInt64?
        let bytes: Data?
    }

    static func varint(_ value: UInt64) -> Data {
        var value = value
        var out = Data()
        while value > 127 {
            out.append(UInt8(value & 127) | 128)
            value >>= 7
        }
        out.append(UInt8(value))
        return out
    }

    static func integer(_ field: Int, _ value: UInt64) -> Data {
        varint(UInt64(field << 3)) + varint(value)
    }

    static func bytes(_ field: Int, _ value: Data) -> Data {
        varint(UInt64(field << 3 | 2)) + varint(UInt64(value.count)) + value
    }

    static func string(_ field: Int, _ value: String) -> Data {
        bytes(field, Data(value.utf8))
    }

    static func float(_ field: Int, _ value: Float) -> Data {
        var bits = value.bitPattern.littleEndian
        return varint(UInt64(field << 3 | 5)) + withUnsafeBytes(of: &bits) { Data($0) }
    }

    static func fields(_ data: Data) throws -> [Field] {
        let data = [UInt8](data)
        var offset = 0
        func readVarint() throws -> UInt64 {
            var value: UInt64 = 0
            for shift in stride(from: 0, through: 63, by: 7) {
                guard offset < data.count else { throw RamblerError.invalid("Truncated protobuf.") }
                let byte = data[offset]
                offset += 1
                guard shift != 63 || byte <= 1 else { throw RamblerError.invalid("Invalid protobuf integer.") }
                value |= UInt64(byte & 127) << shift
                if byte < 128 { return value }
            }
            throw RamblerError.invalid("Invalid protobuf integer.")
        }
        func take(_ count: Int) throws -> Data {
            guard count <= data.count - offset else { throw RamblerError.invalid("Truncated protobuf field.") }
            defer { offset += count }
            return Data(data[offset ..< offset + count])
        }
        var out: [Field] = []
        while offset < data.count {
            let tag = try readVarint()
            guard tag >> 3 > 0, tag >> 3 <= 536_870_911 else {
                throw RamblerError.invalid("Invalid protobuf field number.")
            }
            let number = Int(tag >> 3)
            switch tag & 7 {
            case 0: out.append(Field(number: number, integer: try readVarint(), bytes: nil))
            case 1: _ = try take(8)
            case 2:
                let count = try readVarint()
                guard count <= UInt64(data.count - offset) else { throw RamblerError.invalid("Truncated protobuf message.") }
                out.append(Field(number: number, integer: nil, bytes: try take(Int(count))))
            case 5: _ = try take(4)
            default: throw RamblerError.invalid("Unsupported protobuf wire type.")
            }
        }
        return out
    }
}

struct CallerConfiguration: Decodable {
    let headers: [String: String]

    func validate() throws {
        let expected: Set<String> = ["x-goog-api-key", "x-android-package", "x-android-cert"]
        guard headers.count == expected.count, Set(headers.keys.map { $0.lowercased() }) == expected,
              headers.values.allSatisfy({ !$0.isEmpty && $0.utf8.allSatisfy { (33 ... 126).contains($0) } }) else {
            throw RamblerError.invalid("The private caller configuration is incomplete.")
        }
    }
}

struct Transcription: Codable {
    let text: String
    let voiceEditing: Bool
    let responses: Int
    let completed: Bool
    let grpcStatus: String?
}

enum RamblerProtocol {
    static let endpoint = URL(string: "https://speechs3proto2-pa.googleapis.com/speech.s3.S3StubbyClientConnectorService/S3Bidi")!
    static let routing = "gboard_gemini_v3_streaming_voice_edit_mul"
    static let maxResponse = 16 * 1024 * 1024

    static func header(standard: Bool, locale: String) -> Data {
        var recognizer = Proto.bytes(1, Proto.integer(4, 0))
        for field in [5, 13, 20, 41] { recognizer += Proto.integer(field, 1) }
        recognizer += Proto.integer(7, 2) + Proto.integer(40, 0)
        if !standard { recognizer += Proto.string(50, routing) }
        let audio = Proto.float(2, 16_000) + Proto.integer(3, 10) + Proto.integer(4, 1)
        let user = Proto.bytes(2, Proto.string(1, locale))
        // These fields mirror the traced Android client, including its platform
        // declaration. This is a private-service compatibility experiment.
        let client = Proto.string(2, "gboard") + Proto.string(8, "Android")
            + Proto.string(9, "16") + Proto.string(10, "18.5.3")
            + Proto.string(11, "Pixel") + Proto.integer(26, 1)
        return Proto.string(1, standard ? "recognizer" : "beyond-a2a-recognizer")
            + Proto.integer(2, 0) + Proto.bytes(293100, audio)
            + Proto.bytes(293000, user) + Proto.bytes(294000, client)
            + Proto.bytes(294500, recognizer) + Proto.bytes(1203, Proto.string(1, ""))
    }

    static func frame(_ message: Data) -> Data {
        var length = UInt32(message.count).bigEndian
        return Data([0]) + withUnsafeBytes(of: &length) { Data($0) } + message
    }

    static func request(audio: Data, standard: Bool, locale: String = "en-US") throws -> Data {
        guard audio.count >= 36, audio.count <= 4 * 1024 * 1024,
              audio.starts(with: Data("OggS".utf8)),
              audio.range(of: Data("OpusHead".utf8)) != nil else {
            throw RamblerError.invalid("Choose a mono Ogg Opus recording, up to 4 MiB.")
        }
        var body = frame(header(standard: standard, locale: locale))
        for offset in stride(from: 0, to: audio.count, by: 224) {
            let chunk = audio.subdata(in: offset ..< min(offset + 224, audio.count))
            body += frame(Proto.bytes(293101, Proto.bytes(1, chunk)))
        }
        body += frame(Proto.integer(2, 0) + Proto.integer(3, 1))
        return body
    }

    static func messages(_ data: Data) throws -> [Data] {
        guard data.count <= maxResponse else { throw RamblerError.invalid("Speech response exceeded its limit.") }
        let data = [UInt8](data)
        var offset = 0
        var messages: [Data] = []
        while offset < data.count {
            guard data.count - offset >= 5, data[offset] == 0 else {
                throw RamblerError.invalid("Invalid or compressed gRPC response.")
            }
            let count = data[offset + 1 ..< offset + 5].reduce(0) { ($0 << 8) | Int($1) }
            offset += 5
            guard count <= data.count - offset else { throw RamblerError.invalid("Truncated gRPC response.") }
            messages.append(Data(data[offset ..< offset + count]))
            offset += count
        }
        return messages
    }

    static func result(_ data: Data, standard: Bool, grpcStatus: String?) throws -> Transcription {
        if let status = grpcStatus, status != "0" { throw RamblerError.invalid("Speech service rejected the request (gRPC \(status)).") }
        let messages = try messages(data)
        var completed = false
        var text = ""
        var edited = false
        for message in messages {
            let root = try Proto.fields(message)
            if root.contains(where: { $0.number == 1 && $0.integer == 1 }) { completed = true }
            if root.contains(where: { $0.number == 2 && ($0.integer ?? 0) != 0 })
                || root.contains(where: { $0.number == 1 && ($0.integer ?? 0) > 1 }) {
                throw RamblerError.invalid("The speech service reported an application error.")
            }
            // S3Response -> event -> recognition result -> final result -> first hypothesis.
            for event in root.filter({ $0.number == 1253625 }).compactMap(\.bytes) {
                for recognition in try Proto.fields(event).filter({ $0.number == 1 }).compactMap(\.bytes) {
                    for final in try Proto.fields(recognition).filter({ $0.number == 3 }).compactMap(\.bytes) {
                        guard let hypothesis = try Proto.fields(final).first(where: { $0.number == 3 })?.bytes else { continue }
                        let fields = try Proto.fields(hypothesis)
                        if let edit = fields.first(where: { $0.number == 74956224 })?.bytes,
                           let history = try Proto.fields(edit).first(where: { $0.number == 3 })?.bytes,
                           let value = String(data: history, encoding: .utf8) {
                            text = value
                            edited = true
                        } else if standard, let raw = fields.first(where: { $0.number == 1 })?.bytes,
                                  let value = String(data: raw, encoding: .utf8) {
                            text += (text.isEmpty ? "" : " ") + value
                        }
                    }
                }
            }
        }
        guard completed else { throw RamblerError.invalid("The speech session ended without a completion message.") }
        guard !text.isEmpty else { throw RamblerError.invalid("The service returned no speech. Check the recording and audio format.") }
        guard standard || edited else { throw RamblerError.invalid("The service returned no Rambler editing result.") }
        return Transcription(text: text, voiceEditing: edited, responses: messages.count, completed: completed, grpcStatus: grpcStatus)
    }
}

struct RamblerClient {
    let caller: CallerConfiguration

    func transcribe(_ audio: Data, standard: Bool = false, locale: String = "en-US") async throws -> Transcription {
        try caller.validate()
        var request = URLRequest(url: RamblerProtocol.endpoint, timeoutInterval: 60)
        request.httpMethod = "POST"
        // This Google frontend returned HTML 404 for application/grpc+proto.
        request.setValue("application/grpc", forHTTPHeaderField: "Content-Type")
        request.setValue("trailers", forHTTPHeaderField: "TE")
        request.setValue("identity", forHTTPHeaderField: "grpc-accept-encoding")
        request.setValue("60000000u", forHTTPHeaderField: "grpc-timeout")
        for (name, value) in caller.headers { request.setValue(value, forHTTPHeaderField: name) }
        request.httpBody = try RamblerProtocol.request(audio: audio, standard: standard, locale: locale)
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let response = response as? HTTPURLResponse, response.statusCode == 200,
              response.value(forHTTPHeaderField: "Content-Type")?.hasPrefix("application/grpc") == true else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw RamblerError.invalid("The speech service returned an unexpected HTTP response (\(status)).")
        }
        return try RamblerProtocol.result(data, standard: standard, grpcStatus: response.value(forHTTPHeaderField: "grpc-status"))
    }
}
