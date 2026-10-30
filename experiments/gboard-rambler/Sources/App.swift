import AVFoundation
import UIKit
import UniformTypeIdentifiers

@main
class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(_ application: UIApplication, configurationForConnecting session: UISceneSession, options: UIScene.ConnectionOptions) -> UISceneConfiguration {
        let configuration = UISceneConfiguration(name: "Default", sessionRole: session.role)
        configuration.delegateClass = SceneDelegate.self
        return configuration
    }
}

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        window.rootViewController = RamblerViewController()
        window.makeKeyAndVisible()
        self.window = window
    }
}

@MainActor
final class RamblerViewController: UIViewController, UIDocumentPickerDelegate {
    private let mode = UISegmentedControl(items: ["Rambler", "Standard"])
    private let status = UILabel()
    private let result = UITextView()
    private let sample = UIButton(type: .system)
    private let record = UIButton(type: .system)
    private let choose = UIButton(type: .system)
    private let copy = UIButton(type: .system)
    private var recorder: AVAudioRecorder?
    private var recordingTimer: Timer?
    private var work: Task<Void, Never>?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let title = UILabel()
        title.text = "Rambler PoC"
        title.font = .preferredFont(forTextStyle: .largeTitle)
        let description = UILabel()
        description.text = "Speak naturally. Rambler removes fillers and follows spoken corrections."
        description.numberOfLines = 0
        description.font = .preferredFont(forTextStyle: .body)
        mode.selectedSegmentIndex = 0
        status.text = "Ready"
        status.numberOfLines = 0
        status.accessibilityIdentifier = "speech-status"
        result.font = .preferredFont(forTextStyle: .body)
        result.isEditable = false
        result.backgroundColor = .secondarySystemBackground
        result.layer.cornerRadius = 12
        result.accessibilityIdentifier = "speech-result"
        sample.setTitle("Try correction sample", for: .normal)
        record.setTitle("Start recording", for: .normal)
        choose.setTitle("Choose audio file", for: .normal)
        copy.setTitle("Copy result", for: .normal)
        sample.addTarget(self, action: #selector(runSample), for: .touchUpInside)
        record.addTarget(self, action: #selector(toggleRecording), for: .touchUpInside)
        choose.addTarget(self, action: #selector(chooseAudio), for: .touchUpInside)
        copy.addTarget(self, action: #selector(copyResult), for: .touchUpInside)
        let privacy = UILabel()
        privacy.text = "Audio is sent directly to Google's speech service. This is an experimental client."
        privacy.numberOfLines = 0
        privacy.font = .preferredFont(forTextStyle: .footnote)
        privacy.textColor = .secondaryLabel
        let stack = UIStackView(arrangedSubviews: [title, description, mode, sample, record, choose, status, result, copy, privacy])
        stack.axis = .vertical
        stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 16),
            stack.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -16),
            result.heightAnchor.constraint(greaterThanOrEqualToConstant: 120)
        ])
    }

    private func busy(_ value: Bool) {
        sample.isEnabled = !value
        choose.isEnabled = !value
        mode.isEnabled = !value
        record.isEnabled = !value || recorder != nil
    }

    @objc private func runSample() {
        guard let url = Bundle.main.url(forResource: "sample", withExtension: "wav") else {
            status.text = "The test recording is missing."
            return
        }
        transcribe(url)
    }

    @objc private func copyResult() { UIPasteboard.general.string = result.text }

    @objc private func chooseAudio() {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.audio], asCopy: true)
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        if let url = urls.first { transcribe(url) }
    }

    @objc private func toggleRecording() {
        if let recorder {
            recorder.stop()
            let url = recorder.url
            self.recorder = nil
            recordingTimer?.invalidate()
            recordingTimer = nil
            record.setTitle("Start recording", for: .normal)
            try? AVAudioSession.sharedInstance().setActive(false)
            transcribe(url)
            return
        }
        AVAudioSession.sharedInstance().requestRecordPermission { [weak self] granted in
            Task { @MainActor in
                guard let self else { return }
                guard granted else { self.status.text = "Allow microphone access in Settings to record."; return }
                do {
                    let session = AVAudioSession.sharedInstance()
                    try session.setCategory(.record, mode: .measurement)
                    try session.setActive(true)
                    let url = FileManager.default.temporaryDirectory.appendingPathComponent("dictation.wav")
                    let recorder = try AVAudioRecorder(url: url, settings: [
                        AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 16_000,
                        AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 16,
                        AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false
                    ])
                    guard recorder.record() else { throw RamblerError.invalid("The microphone could not start.") }
                    self.recorder = recorder
                    self.record.setTitle("Finish recording", for: .normal)
                    self.status.text = "Recording…"
                    self.busy(true)
                    self.recordingTimer = Timer.scheduledTimer(withTimeInterval: 120, repeats: false) { [weak self] _ in
                        Task { @MainActor in self?.toggleRecording() }
                    }
                } catch {
                    try? AVAudioSession.sharedInstance().setActive(false)
                    self.status.text = error.localizedDescription
                }
            }
        }
    }

    private func transcribe(_ url: URL) {
        guard work == nil else { return }
        let standard = mode.selectedSegmentIndex == 1
        busy(true)
        result.text = ""
        status.text = "Transcribing…"
        work = Task {
            defer { busy(false); work = nil }
            do {
                guard let configuration = Bundle.main.url(forResource: "caller", withExtension: "json") else {
                    throw RamblerError.invalid("The build is missing its private caller configuration.")
                }
                let caller = try JSONDecoder().decode(CallerConfiguration.self, from: Data(contentsOf: configuration))
                let audio = try await Task.detached {
                    try url.pathExtension.lowercased() == "ogg" ? Data(contentsOf: url) : OggOpus.encode(file: url)
                }.value
                let transcription = try await RamblerClient(caller: caller).transcribe(audio, standard: standard)
                result.text = transcription.text
                status.text = standard ? "Standard transcription" : "Rambler correction received"
                // Local test evidence contains no caller headers or credentials.
                let evidence = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("last-result.json")
                try? JSONEncoder().encode(transcription).write(to: evidence, options: .atomic)
            } catch {
                status.text = error.localizedDescription
            }
        }
    }
}
