import Foundation

@main
struct RamblerCLI {
    static func main() async {
        let args = CommandLine.arguments
        guard args.count >= 3 else {
            print("Usage: rambler-poc CALLER.json AUDIO.wav-or-ogg [--standard]")
            exit(2)
        }
        do {
            let caller = try JSONDecoder().decode(CallerConfiguration.self, from: Data(contentsOf: URL(fileURLWithPath: args[1])))
            let url = URL(fileURLWithPath: args[2])
            let audio = try url.pathExtension.lowercased() == "ogg" ? Data(contentsOf: url) : OggOpus.encode(file: url)
            let result = try await RamblerClient(caller: caller).transcribe(audio, standard: args.contains("--standard"))
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            print(String(decoding: try encoder.encode(result), as: UTF8.self))
        } catch {
            // Never log URLRequests, HTTP headers or the private configuration.
            print("Failed: \(error.localizedDescription)")
            exit(1)
        }
    }
}
