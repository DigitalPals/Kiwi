import Foundation

@main
struct ProtocolTests {
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) {
        guard condition() else { fatalError(message) }
    }

    static func rejected(_ name: String, _ body: () throws -> Void) {
        do { try body(); fatalError("Accepted invalid input: \(name)") }
        catch { }
    }

    static func response(text: String, edited: Bool) -> Data {
        var hypothesis = Proto.string(1, text)
        if edited { hypothesis += Proto.bytes(74956224, Proto.string(3, text)) }
        return Proto.bytes(1253625, Proto.bytes(1, Proto.integer(1, 1) + Proto.bytes(3, Proto.bytes(3, hypothesis))))
    }

    static func main() throws {
        let text = "Meet on Wednesday."
        let final = response(text: text, edited: true)
        let completed = RamblerProtocol.frame(Proto.integer(1, 1))
        let stream = RamblerProtocol.frame(final) + completed
        let result = try RamblerProtocol.result(stream, standard: false, grpcStatus: nil)
        require(result.text == text && result.voiceEditing && result.completed, "Lost the server's edited history")
        require(result.responses == 2, "Lost a gRPC frame")
        let control = try RamblerProtocol.result(RamblerProtocol.frame(response(text: "Tuesday actually no Wednesday", edited: false)) + completed, standard: true, grpcStatus: "0")
        require(!control.voiceEditing && control.text.contains("Tuesday"), "Incorrect standard transcript")
        rejected("legacy transcript passed off as Rambler") {
            _ = try RamblerProtocol.result(RamblerProtocol.frame(response(text: text, edited: false)) + completed, standard: false, grpcStatus: nil)
        }
        rejected("missing terminal message") { _ = try RamblerProtocol.result(RamblerProtocol.frame(final), standard: false, grpcStatus: nil) }
        rejected("empty edit") { _ = try RamblerProtocol.result(RamblerProtocol.frame(response(text: "", edited: true)) + completed, standard: false, grpcStatus: nil) }
        rejected("gRPC permission denial") { _ = try RamblerProtocol.result(stream, standard: false, grpcStatus: "7") }
        rejected("application error") { _ = try RamblerProtocol.result(RamblerProtocol.frame(Proto.integer(2, 1)) + stream, standard: false, grpcStatus: "0") }
        rejected("truncated frame") { _ = try RamblerProtocol.messages(Data([0, 0, 0, 0, 9, 1])) }
        rejected("compressed frame") { _ = try RamblerProtocol.messages(Data([1, 0, 0, 0, 0])) }
        rejected("overflowed varint") { _ = try Proto.fields(Data(repeating: 255, count: 10)) }
        rejected("truncated length") { _ = try Proto.fields(Data([10, 10, 0])) }
        rejected("zero tag") { _ = try Proto.fields(Data([0, 0])) }
        rejected("invalid audio") { _ = try RamblerProtocol.request(audio: Data("not audio".utf8), standard: false) }
        rejected("unexpected credential header") { try CallerConfiguration(headers: ["Authorization": "example"]).validate() }
        rejected("header injection") {
            try CallerConfiguration(headers: ["x-goog-api-key": "example\r\nvalue", "x-android-package": "example", "x-android-cert": "example"]).validate()
        }
        let unknown = Proto.integer(900, 1) + Proto.bytes(901, Data([0, 255]))
        let unknownFields = try Proto.fields(unknown)
        require(unknownFields.count == 2, "Unknown fields should remain readable")
        print("Passed: edited results, control results, completion, errors, framing, malformed protobuf and caller validation.")
    }
}
