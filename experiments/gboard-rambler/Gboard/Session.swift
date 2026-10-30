import AVFoundation
import UIKit

// The Android-compatible service client is shared with the tested standalone app.
// Only recording and delivery to Gboard's existing controller are replaced here.
@MainActor @objc(KiwiGboardRamblerSession)
final class GboardRamblerSession: NSObject {
    private weak var controller: NSObject?
    private var recorder: AVAudioRecorder?
    private var timer: Timer?
    private var interruption: NSObjectProtocol?
    private var work: Task<Void, Never>?
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var recording: URL?
    private var generation = UUID()
    private var started = Date()
    private var speechDetector: StreamSpeechDetector?
    private var endpoint = SpeechEndpoint()
    private var reportedClassifier = false
    private var nextMeterTrace = 0.0
    private var standard = false
    @objc private(set) var active = false
    @objc private(set) var capturing = false
    @objc private(set) var microphoneReady = false
    @objc private(set) var text = ""
    @objc private(set) var level = 0.0

    @objc(initWithController:)
    init(controller: NSObject) { self.controller = controller }

    private var root: Bundle { Bundle.main }

    private func trace(_ event: String, _ fields: [String: Any] = [:]) {
        var state = fields
        state["session"] = String(generation.uuidString.prefix(8))
        state["app_state"] = UIApplication.shared.applicationState.rawValue
        KGRRecordEvent(event, state)
    }

    @objc func start() {
        guard !active else { trace("capture_already_active"); return }
        generation = UUID()
        let current = generation
        text = ""
        active = true
        endpoint = SpeechEndpoint()
        reportedClassifier = false
        microphoneReady = false
        KGRPublishCaptureState(false)
        trace("capture_requested", ["experimental": KGRPreferences().bool(forKey: "KiwiRamblerInKeyboard")])
        standard = KGRPreferences().bool(forKey: "KiwiRamblerStandard")
        KGRPreferences().set("finish", forKey: "KiwiRamblerStopAction")
        KGRPreferences().synchronize()
        if let controller { KGRShowStatus(controller, standard ? "Listening…" : "Rambler · Listening…") }
        #if targetEnvironment(simulator)
        // Used by the native integration harness; never included in device builds.
        if let path = ProcessInfo.processInfo.environment["KIWI_RAMBLER_TEST_AUDIO"] {
            recording = URL(fileURLWithPath: path)
            level = 0.42
            let delay = Double(ProcessInfo.processInfo.environment["KIWI_RAMBLER_TEST_CAPTURE_DELAY"] ?? "0") ?? 0
            if delay == 0 { capturing = true; microphoneReady = true; KGRPublishCaptureState(true) }
            else {
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                    guard let self, self.generation == current, self.active else { return }
                    self.capturing = true
                    self.microphoneReady = true
                    KGRPublishCaptureState(true)
                }
            }
            return
        }
        #endif
        AVAudioSession.sharedInstance().requestRecordPermission { [weak self] granted in
            Task { @MainActor in
                guard let self, self.generation == current, self.active else { return }
                self.trace("microphone_permission", ["granted": granted])
                guard granted else { self.fail("Allow Gboard microphone access in Settings.", reason: "permission_denied"); return }
                do {
                    let session = AVAudioSession.sharedInstance()
                    // Gboard's monitor requires this category for its background bridge.
                    let options: AVAudioSession.CategoryOptions = session.category == .playAndRecord
                        ? session.categoryOptions.union(.mixWithOthers) : [.mixWithOthers]
                    try session.setCategory(.playAndRecord, mode: .measurement, options: options)
                    try session.setActive(true)
                    let url = FileManager.default.temporaryDirectory.appendingPathComponent("kiwi-rambler-\(current).wav")
                    let recorder = try AVAudioRecorder(url: url, settings: [
                        AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 16_000,
                        AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 16,
                        AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false
                    ])
                    recorder.isMeteringEnabled = true
                    guard recorder.record() else { throw RamblerError.invalid("Gboard could not start its microphone.") }
                    self.recording = url
                    self.recorder = recorder
                    self.capturing = true
                    // record() accepting the request is not evidence that the input
                    // clock has started. Wait for recorded frames before leaving.
                    self.microphoneReady = false
                    self.started = Date()
                    self.nextMeterTrace = 0
                    self.speechDetector = StreamSpeechDetector(url: url) { [weak self] event in
                        Task { @MainActor in
                            guard let self, self.generation == current, self.active, self.capturing else { return }
                            switch event {
                            case .classification(let start, let end, let speech, let wind):
                                self.endpoint.classify(start: start, end: end, speech: speech, wind: wind)
                                if !self.reportedClassifier {
                                    self.reportedClassifier = true
                                    self.trace("speech_detector_ready", ["window_seconds": 0.5, "pause_seconds": SpeechEndpoint.pause])
                                }
                            case .unavailable:
                                self.trace("speech_detector_fallback")
                            }
                        }
                    }
                    self.trace("capture_started", ["category": session.category.rawValue,
                        "mode": session.mode.rawValue, "options": session.categoryOptions.rawValue,
                        "sample_rate": session.sampleRate, "recording_sample_rate": recorder.format.sampleRate,
                        "input_types": session.currentRoute.inputs.map { $0.portType.rawValue }])
                    self.interruption = NotificationCenter.default.addObserver(
                        forName: AVAudioSession.interruptionNotification, object: session, queue: .main
                    ) { [weak self] notification in
                        let type = (notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? NSNumber)?.uintValue
                        Task { @MainActor in
                            guard let self, self.active else { return }
                            self.trace("audio_interruption", ["type": type ?? 0])
                            if type == AVAudioSession.InterruptionType.began.rawValue {
                                self.fail("Dictation was interrupted. Tap the microphone to try again.", reason: "audio_interruption")
                            }
                        }
                    }
                    let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
                        Task { @MainActor in self?.meter() }
                    }
                    self.timer = timer
                    RunLoop.main.add(timer, forMode: .common)
                } catch { self.fail(error.localizedDescription, reason: "capture_setup", error: error) }
            }
        }
    }

    private func meter() {
        guard active, let recorder else { return }
        guard recorder.isRecording else {
            trace("capture_lost", ["recorded_seconds": recorder.currentTime])
            fail("The microphone stopped recording. Open Gboard and try again with Dictate in keyboard turned off.", reason: "recorder_stopped")
            return
        }
        recorder.updateMeters()
        let power = recorder.averagePower(forChannel: 0)
        if !microphoneReady && recorder.currentTime >= 0.05 {
            microphoneReady = true
            KGRPublishCaptureState(true)
            trace("capture_ready", ["recorded_seconds": recorder.currentTime])
        }
        // Gboard's overlay expects a perceptual 0...1 meter. Raw linear
        // amplitude made ordinary -36 dB speech look almost silent (0.016).
        level = power.isFinite ? min(1, max(0, (Double(power) + 60) / 50)) : 0
        let recorded = recorder.currentTime
        endpoint.meter(power: Double(power), time: recorded)
        let now = ProcessInfo.processInfo.systemUptime
        if now >= nextMeterTrace {
            KGRPublishCaptureState(microphoneReady)
            trace("capture_meter", ["recorded_seconds": recorded,
                "power_db": power.isFinite ? Int(power) : -160, "speech_detected": endpoint.heardSpeech,
                "speech_confidence": endpoint.confidence, "classified_through": endpoint.classifiedThrough,
                "noise_floor_db": Int(endpoint.noiseFloor)])
            nextMeterTrace = now + 1
        }
        let elapsed = Date().timeIntervalSince(started)
        if elapsed >= 120 || endpoint.shouldFinish(time: recorded) {
            complete(reason: elapsed >= 120 ? "limit" : "speech_pause")
        } else if !endpoint.heardSpeech && elapsed >= 15 {
            fail("No speech was detected. Tap the microphone to try again.", reason: "no_speech")
        }
    }

    private func stopCapture() {
        if capturing { trace("capture_stopped", ["recorded_seconds": recorder?.currentTime ?? 0]) }
        timer?.invalidate(); timer = nil
        speechDetector?.stop(); speechDetector = nil
        if let interruption { NotificationCenter.default.removeObserver(interruption) }
        interruption = nil
        recorder?.stop(); recorder = nil
        capturing = false
        microphoneReady = false
        KGRPublishCaptureState(false)
        level = 0
    }

    private func endBackgroundWork() {
        guard backgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTask)
        backgroundTask = .invalid
    }

    private func removeRecording() {
        #if targetEnvironment(simulator)
        if ProcessInfo.processInfo.environment["KIWI_RAMBLER_TEST_AUDIO"] != nil { recording = nil; return }
        #endif
        if let recording { try? FileManager.default.removeItem(at: recording) }
        recording = nil
    }

    @objc func finish() { complete(reason: "stop") }

    private func complete(reason: String) {
        guard active, work == nil else { return }
        guard let url = recording else { cancel(); return }
        trace("transcription_requested", ["recorded_seconds": recorder?.currentTime ?? 0,
            "speech_detected": endpoint.heardSpeech, "finish_reason": reason])
        // Stop in Gboard's background monitor immediately tears down its strong
        // controller reference. Keep the original delivery target alive until
        // this asynchronous result/error is delivered, including that path.
        let deliveryController = controller
        // Once recording stops, background audio no longer keeps the host alive.
        // Hold a bounded system task for encoding, the request and result delivery.
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Rambler transcription") { [weak self] in
            Task { @MainActor in self?.fail("iOS ended background dictation. Open Gboard and try again.", reason: "background_expired") }
        }
        stopCapture()
        let current = generation
        let standard = standard
        let locale = controller.map { KGRLanguage($0) } ?? "en-US"
        if let controller { KGRShowStatus(controller, standard ? "Transcribing…" : "Rambler · Correcting…") }
        work = Task {
            do {
                guard let file = root.url(forResource: "KiwiRamblerCaller", withExtension: "json") else {
                    throw RamblerError.invalid("The Gboard build is missing its Rambler caller configuration.")
                }
                let caller = try JSONDecoder().decode(CallerConfiguration.self, from: Data(contentsOf: file))
                let encodingStarted = ProcessInfo.processInfo.systemUptime
                let encoded = try await Task.detached { try DictationAudio.encode(file: url) }.value
                try Task.checkCancellation()
                trace("speech_request_started", ["encoded_bytes": encoded.audio.count,
                    "original_seconds": encoded.originalSeconds, "submitted_seconds": encoded.submittedSeconds,
                    "audio_start_seconds": 0,
                    "encoding_seconds": ProcessInfo.processInfo.systemUptime - encodingStarted])
                let requestStarted = ProcessInfo.processInfo.systemUptime
                let result = try await RamblerClient(caller: caller).transcribe(encoded.audio, standard: standard, locale: locale)
                try Task.checkCancellation()
                guard generation == current, active else { return }
                text = result.text
                active = false
                work = nil
                removeRecording()
                trace("speech_request_finished", ["characters": result.text.count, "delivery_target": deliveryController != nil,
                    "request_seconds": ProcessInfo.processInfo.systemUptime - requestStarted])
                if let deliveryController { KGRDeliverResult(deliveryController, result.text) }
                endBackgroundWork()
            } catch {
                guard generation == current, active else { return }
                fail(error.localizedDescription, reason: "transcription", error: error, deliveryTarget: deliveryController)
            }
        }
    }

    @objc func cancel() {
        if active || work != nil { trace("capture_cancelled") }
        generation = UUID()
        active = false
        microphoneReady = false
        work?.cancel(); work = nil
        stopCapture()
        removeRecording()
        endBackgroundWork()
        text = ""
    }

    @objc func handoffTimedOut() {
        fail("The microphone was not ready. Open Gboard and try dictation again.", reason: "handoff_timeout")
    }

    private func fail(_ message: String, reason: String, error: Error? = nil, deliveryTarget: NSObject? = nil) {
        let target = deliveryTarget ?? controller
        var state: [String: Any] = ["reason": reason]
        if let error { state["error_domain"] = (error as NSError).domain; state["error_code"] = (error as NSError).code }
        trace("failure", state)
        cancel()
        if let target { KGRDeliverError(target, NSError(domain: "com.kiwi.gboard.rambler", code: 1,
            userInfo: [NSLocalizedDescriptionKey: message])) }
    }
}

@MainActor @objc(KiwiGboardRamblerSettings)
final class GboardRamblerSettings: UIViewController {
    private let mode = UISegmentedControl(items: ["Rambler", "Standard"])
    private let result = UITextView()
    private let sample = UIButton(type: .system)
    private var task: Task<Void, Never>?

    @objc static func presentFrom(_ controller: UIViewController) {
        guard controller.presentedViewController == nil else { return }
        controller.present(UINavigationController(rootViewController: GboardRamblerSettings()), animated: true)
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Rambler"
        view.backgroundColor = .systemBackground
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        let info = UILabel()
        info.numberOfLines = 0
        info.font = .preferredFont(forTextStyle: .body)
        info.text = "Use Gboard’s microphone to dictate. Rambler removes fillers and applies spoken corrections. Pause to finish or tap Stop. Audio is sent to Google. No personal API key is needed."
        let background = UISwitch()
        background.accessibilityIdentifier = "kiwi.rambler.in-keyboard"
        background.isOn = KGRPreferences().bool(forKey: "KiwiRamblerInKeyboard")
        background.addAction(UIAction { [weak background] _ in
            guard let background else { return }
            KGRPreferences().set(background.isOn, forKey: "KiwiRamblerInKeyboard")
            KGRPreferences().synchronize()
        }, for: .valueChanged)
        let backgroundTitle = UILabel()
        backgroundTitle.text = "Dictate in keyboard (experimental)"
        backgroundTitle.font = .preferredFont(forTextStyle: .body)
        backgroundTitle.numberOfLines = 0
        let backgroundRow = UIStackView(arrangedSubviews: [backgroundTitle, background])
        backgroundRow.spacing = 12
        let backgroundInfo = UILabel()
        backgroundInfo.font = .preferredFont(forTextStyle: .footnote)
        backgroundInfo.textColor = .secondaryLabel
        backgroundInfo.numberOfLines = 0
        backgroundInfo.text = "iOS requires opening Gboard to start the microphone. This mode then records while you use the original app. If iOS cannot return automatically, switch back yourself before speaking. Recording ends when you pause or tap Stop; the microphone is not kept on between dictations. Turn this off if dictation fails."
        mode.selectedSegmentIndex = KGRPreferences().bool(forKey: "KiwiRamblerStandard") ? 1 : 0
        mode.addAction(UIAction { [weak self] _ in
            KGRPreferences().set(self?.mode.selectedSegmentIndex == 1, forKey: "KiwiRamblerStandard")
            KGRPreferences().synchronize()
        }, for: .valueChanged)
        sample.setTitle("Test spoken correction", for: .normal)
        sample.addAction(UIAction { [weak self] _ in self?.testSample() }, for: .touchUpInside)
        result.isEditable = false
        result.font = .preferredFont(forTextStyle: .body)
        result.text = KGRLastDictation()
        let copy = UIButton(type: .system)
        copy.setTitle("Copy text", for: .normal)
        copy.accessibilityIdentifier = "kiwi.rambler.copy"
        copy.addAction(UIAction { [weak self, weak copy] _ in
            guard let text = self?.result.text, !text.isEmpty else { return }
            UIPasteboard.general.string = text
            copy?.setTitle("Copied", for: .normal)
        }, for: .touchUpInside)
        let diagnostics = UIButton(type: .system)
        diagnostics.setTitle("Diagnostics", for: .normal)
        diagnostics.accessibilityIdentifier = "kiwi.rambler.diagnostics"
        diagnostics.addAction(UIAction { [weak self] _ in
            self?.navigationController?.pushViewController(GboardRamblerDiagnosticsViewController(), animated: true)
        }, for: .touchUpInside)
        let stack = UIStackView(arrangedSubviews: [info, mode, backgroundRow, backgroundInfo, sample, diagnostics, result, copy])
        stack.axis = .vertical; stack.spacing = 20
        stack.translatesAutoresizingMaskIntoConstraints = false
        let scroll = UIScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scroll)
        scroll.addSubview(stack)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -20),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 20),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -20),
            stack.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor, constant: -40),
            result.heightAnchor.constraint(greaterThanOrEqualToConstant: 100)
        ])
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        task?.cancel()
    }

    private func testSample() {
        guard task == nil else { return }
        sample.isEnabled = false
        result.text = "Checking Rambler…"
        task = Task {
            defer { sample.isEnabled = true; task = nil }
            do {
                guard let config = Bundle.main.url(forResource: "KiwiRamblerCaller", withExtension: "json"),
                      let audio = Bundle.main.url(forResource: "KiwiRamblerSample", withExtension: "wav") else {
                    throw RamblerError.invalid("The build is missing the sample or caller configuration.")
                }
                let caller = try JSONDecoder().decode(CallerConfiguration.self, from: Data(contentsOf: config))
                let data = try await Task.detached { try OggOpus.encode(file: audio) }.value
                let response = try await RamblerClient(caller: caller).transcribe(data)
                try Task.checkCancellation()
                result.text = response.text
            } catch { result.text = error.localizedDescription }
        }
    }
}

@MainActor
private final class GboardRamblerDiagnosticsViewController: UIViewController {
    private let report = UITextView()

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Rambler diagnostics"
        view.backgroundColor = .systemBackground
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Copy report", style: .plain, target: self, action: #selector(copyReport))
        let info = UILabel()
        info.numberOfLines = 0
        info.font = .preferredFont(forTextStyle: .footnote)
        info.text = "Recording and keyboard handoff events only. Speech, transcripts and keys are excluded. Copy this report after a failed attempt."
        let refreshButton = UIButton(type: .system)
        refreshButton.setTitle("Refresh", for: .normal)
        refreshButton.addAction(UIAction { [weak self] _ in self?.refresh() }, for: .touchUpInside)
        let clear = UIButton(type: .system)
        clear.setTitle("Clear report", for: .normal)
        clear.addAction(UIAction { [weak self] _ in KGRClearDiagnostics(); self?.refresh() }, for: .touchUpInside)
        let buttons = UIStackView(arrangedSubviews: [refreshButton, clear])
        buttons.spacing = 24
        report.isEditable = false
        report.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        report.accessibilityIdentifier = "kiwi.rambler.diagnostics.report"
        let stack = UIStackView(arrangedSubviews: [info, buttons, report])
        stack.axis = .vertical; stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 16),
            stack.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -16)
        ])
        refresh()
    }

    private func refresh() {
        report.text = KGRDiagnosticsReport()
        navigationItem.rightBarButtonItem?.title = "Copy report"
    }

    @objc private func copyReport() {
        refresh()
        UIPasteboard.general.string = report.text
        navigationItem.rightBarButtonItem?.title = "Copied"
    }
}
