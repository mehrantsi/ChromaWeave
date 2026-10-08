#if CW_DEVELOPMENT
import AVFoundation
import UIKit
import UniformTypeIdentifiers
import ImageIO

enum DevelopmentReplay {
    static func configuration(arguments: [String] = ProcessInfo.processInfo.arguments) -> CameraConfiguration {
        let flags = ["--live-stream-self-test", "--transfer-self-test", "--transfer-repair-self-test"]
        return flags.contains(where: arguments.contains) ? .savedFrames : .camera
    }
    static func start(scanner: CameraScanner, arguments: [String] = ProcessInfo.processInfo.arguments) -> Bool {
        if arguments.contains("--scan-diagnostics-self-test") {
            CameraScanner.diagnosticsSelfTest { result in
                do {
                    let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                    try result.get().write(to: directory.appendingPathComponent("diagnostics-selftest.json"), options: .atomic)
                    scanner.status = "Diagnostics self-test passed"
                } catch { scanner.status = "Diagnostics self-test failed: \(error.localizedDescription)" }
            }
        } else if arguments.contains("--transfer-repair-self-test") {
            scanner.transferSelfTest(repair: true)
        } else if arguments.contains("--transfer-self-test") {
            scanner.transferSelfTest()
        } else if arguments.contains("--photo-capture-self-test") {
            scanner.photoSelfTest()
        } else if arguments.contains("--camera-frame-self-test") {
            scanner.cameraFrameSelfTest()
        } else if arguments.contains("--live-stream-self-test") {
            scanner.liveStreamSelfTest()
        } else if arguments.contains("--recovery-camera-self-test") {
            scanner.recoveryCameraSelfTest()
        } else if arguments.contains("--codec-self-test") {
            let kind = arguments.firstIndex(of: "--preview-sample").flatMap { index in
                index + 1 < arguments.count ? CodeSample(rawValue: arguments[index + 1]) : nil
            } ?? .binary
            scanner.codecSelfTest(kind, profile: arguments.contains("--low-ecc") ? 4 : Codec.defaultProfile)
        } else { return false }
        return true
    }

}

extension CameraScanner {
    func photoSelfTest() {
        let ticket = beginImage("Checking still-photo preparation…")
        decodeQueue.async { [weak self] in
            let output = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                .appendingPathComponent("photo-selftest.json")
            do {
                let preparation = try Codec.photoSelfTest()
                var report = try JSONSerialization.jsonObject(with: preparation) as! [String: Any]
                report.merge(try CapturedPhotoArchive.selfTest()) { _, value in value }
                report["transfer_receiver"] = try TransferReceiver.selfTest()
                try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted]).write(to: output, options: .atomic)
                DispatchQueue.main.async {
                    guard let self, self.state.isCurrent(ticket) else { return }
                    self.processingImage = false
                    self.status = "Photo preparation self-test passed"
                    self.diagnostic = "Photo preparation and paired archive verified, including disk-failure handling. Physical camera not exercised."
                }
            } catch {
                let report = ["passed": false, "error": error.localizedDescription] as [String: Any]
                try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted]).write(to: output, options: .atomic)
                self?.failed(error, ticket: ticket)
            }
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer) {
        guard let frame = decoding.videoFrame(sampleBuffer) else { return }
        decoding.processVideoFrame(frame)
    }

    func codecSelfTest(_ kind: CodeSample = .binary, selfTest: Bool = true, profile: Int = Codec.defaultProfile) {
        let ticket = beginImage("Reading \(kind.title.lowercased())…")
        decodeQueue.async { [weak self] in
            do {
                var selected: DecodedPayload?
                var reports = [[String: Any]]()
                for sample in selfTest ? CodeSample.allCases : [kind] {
                    let payload = sample.data
                    let encoded = try Codec.encode(payload, profile: profile, cellPixels: 2)
                    let decoded = try Codec.decode(image: encoded.image)
                    guard decoded.data == payload else { throw CWFailure(message: "Sample payload mismatch.") }
                    if sample == kind { selected = decoded }
                    if selfTest {
                        let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                        try encoded.png.write(to: directory.appendingPathComponent("selftest-\(sample.rawValue)-code.png"), options: .atomic)
                        try decoded.data.write(to: directory.appendingPathComponent("selftest-\(sample.rawValue)-payload.\(PayloadContent(data: payload).fileExtension)"), options: .atomic)
                        reports.append(["sample": sample.rawValue, "exact": true, "bytes": decoded.data.count,
                                        "profile": decoded.profile, "sidePixels": encoded.side,
                                        "rust_final_call_ms": decoded.finalCallMilliseconds])
                        NSLog("CW_SELFTEST_OK sample=\(sample.rawValue) bytes=\(decoded.data.count) profile=\(decoded.profile)")
                    }
                }
                if selfTest {
                    let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                    try JSONSerialization.data(withJSONObject: reports, options: .prettyPrinted)
                        .write(to: directory.appendingPathComponent("selftest.json"), options: .atomic)
                }
                if let selected { self?.accept(selected, ticket: ticket) }
            } catch {
                if selfTest { NSLog("CW_SELFTEST_FAILED \(error.localizedDescription)") }
                self?.failed(error, ticket: ticket)
            }
        }
    }

    static func diagnosticsSelfTest(completion: @escaping (Result<Data, Error>) -> Void) {
        DispatchQueue.global(qos: .utility).async {
            let result = Result { try ScanDiagnosticStore.selfTest() }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func cameraFrameSelfTest() {
        let ticket = beginImage("Reading camera regression image…")
        decodeQueue.async { [weak self] in
            let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            let output = directory.appendingPathComponent("camera-regression-result.json")
            let started = CACurrentMediaTime()
            var report: [String: Any] = ["exact": false,
                "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown"]
            do {
                let imageData = try Data(contentsOf: directory.appendingPathComponent("camera-regression.png"))
                guard let image = UIImage(data: imageData) else { throw CWFailure(message: "Cannot read camera regression image.") }
                let decoded = try Codec.decode(image: image)
                let expected = try Data(contentsOf: directory.appendingPathComponent("camera-regression-payload.bin"))
                report["bytes"] = decoded.data.count
                report["profile"] = decoded.profile
                report["rust_final_call_ms"] = decoded.finalCallMilliseconds
                report["corrected"] = decoded.corrected
                report["erased"] = decoded.erased
                guard decoded.data == expected else { throw CWFailure(message: "Camera regression payload mismatch.") }
                let content = PayloadContent(data: decoded.data)
                if content.isGIF {
                    guard content.gifFrameCount > 0 else { throw CWFailure(message: "Cannot read recovered GIF frames.") }
                    report["payloadLabel"] = content.label
                    report["fileExtension"] = content.fileExtension
                    report["mimeType"] = content.inertImageMIMEType
                    report["gifFrameCount"] = content.gifFrameCount
                    let exported = directory.appendingPathComponent("camera-regression-export.gif")
                    try content.data.write(to: exported, options: .atomic)
                    report["exportExact"] = try Data(contentsOf: exported) == expected
                }
                report["exact"] = true
                report["elapsedMilliseconds"] = (CACurrentMediaTime() - started) * 1000
                try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted])
                    .write(to: output, options: .atomic)
                self?.accept(decoded, ticket: ticket)
            } catch {
                report["exact"] = false
                report["error"] = error.localizedDescription
                report["elapsedMilliseconds"] = (CACurrentMediaTime() - started) * 1000
                do {
                    try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted])
                        .write(to: output, options: .atomic)
                } catch {
                    self?.failed(CWFailure(message: "Cannot save camera regression result: \(error.localizedDescription)"), ticket: ticket)
                    return
                }
                self?.failed(error, ticket: ticket)
            }
        }
    }

    func transferSelfTest(repair: Bool = false) {
        _ = beginImage("Checking the buffered transfer path…")
        liveTestSuppressPayload = true
        liveTestPayload = nil
        decodeQueue.async { [weak self] in
            guard let self else { return }
            let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            let fixtures = directory.appendingPathComponent(repair ? "transfer-repair-regression" : "transfer-regression")
            let reportURL = directory.appendingPathComponent(repair ? "transfer-repair-selftest-result.json" : "transfer-selftest-result.json")
            var report: [String: Any] = ["passed": false, "capture_source": "saved_bgra_provider",
                "hardware_capture_verified": false, "physical_throughput_verified": false,
                "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown"]
            do {
                report["receiver"] = try TransferReceiver.selfTest()
                let expected = try Data(contentsOf: fixtures.appendingPathComponent("source.bin"))
                let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: fixtures.appendingPathComponent("manifest.json"))) as? [String: Any]
                guard let frames = manifest?["frames"] as? [[String: Any]], frames.count == (repair ? 7 : 3) else {
                    throw CWFailure(message: "Transfer regression has the wrong number of chunk images.")
                }
                let samples = try frames.enumerated().map { index, frame in
                    guard let filename = frame["filename"] as? String else { throw CWFailure(message: "Missing transfer image.") }
                    return try Codec.savedImageSampleBuffer(Data(contentsOf: fixtures.appendingPathComponent(filename)),
                        timestamp: CMTime(seconds: Double(index + 1), preferredTimescale: 600))
                }
                self.state.lock.withLock {
                    self.state.cameraWanted = true
                    self.state.automaticDecoding = true
                    self.state.transferDecoding = true
                }
                for index in [0, 1, 2, 0] { self.decoding.enqueueVideoFrame(samples[index]) }
                let bounded = self.state.lock.withLock {
                    self.decoding.pendingVideoFrames.count == 2 && self.decoding.videoBuffersOverwritten == 2 &&
                        self.decoding.pendingVideoFrames.map(\.sensorTimestamp) == [3, 1] && self.decoding.videoWorkerScheduled
                }
                guard bounded else { throw CWFailure(message: "The transfer queue exceeded its bound or retained the wrong frames.") }
                report["bounded_two_pending_newest_frames"] = true
                self.state.lock.withLock { self.state.captureRequested = true }
                self.decoding.enqueueVideoFrame(samples[1])
                self.decoding.enqueueVideoFrame(samples[2])
                let protected = self.state.lock.withLock {
                    self.decoding.pendingVideoFrames.count == 1 && self.decoding.pendingVideoFrames[0].capture &&
                        self.decoding.pendingVideoFrames[0].sensorTimestamp == 2
                }
                guard protected else { throw CWFailure(message: "The manual capture was overwritten.") }
                report["manual_capture_preserved"] = true
                let stale = self.state.lock.withLock { self.decoding.pendingVideoFrames[0] }
                self.stop()
                self.decoding.processVideoFrame(stale)
                let cancelled = self.state.lock.withLock { self.decoding.pendingVideoFrames.isEmpty && !self.state.cameraWanted }
                guard cancelled, self.decoding.transferReceiver.transferIDHex == nil else { throw CWFailure(message: "Cancelled camera work was retained.") }
                report["cancellation_discards_pending_and_stale_frame"] = true
                let ticket = self.state.lock.withLock { () -> Int in
                    self.state.generation += 1
                    self.state.cameraWanted = true
                    return self.state.generation
                }
                self.decoding.reset(for: ticket)
                let output = AVCaptureVideoDataOutput()
                let frameOrder = repair ? [3, 3, 4, 5] : [2, 0, 0, 1]
                for position in frameOrder.indices {
                    if position == 2 { self.state.lock.withLock { self.state.lensRevision += 1 } }
                    self.captureOutput(output, didOutput: samples[frameOrder[position]])
                }
                guard self.decoding.transferScannerSession != nil,
                      self.decoding.transferScannerLensRevision == self.state.lock.withLock({ self.state.lensRevision }) else {
                    throw CWFailure(message: "Transfer tracking did not follow the camera revision.")
                }
                report["transfer_scanner_context_created"] = true
                report["lens_revision_reset_exercised"] = true
                report["duplicate_expected"] = 1
                report["decode_ticket"] = ticket
                report["generation_at_decode_completion"] = self.state.lock.withLock { self.state.generation }
                DispatchQueue.main.async {
                    report["generation_at_report_delivery"] = self.state.lock.withLock { self.state.generation }
                    report["exact"] = self.liveTestPayload?.data == expected
                    let statistics = self.liveTestPayload?.transfer
                    let finalRate = statistics?.bytes == expected.count && statistics?.duplicates == 1 &&
                        (statistics?.megabitsPerSecond ?? 0) > 0 && (statistics?.elapsedSeconds ?? 0) > 0
                    let repaired = !repair || statistics?.repairedChunks == 3 && statistics?.repairSymbols == 3
                    report["repaired_chunks"] = statistics?.repairedChunks
                    report["repair_symbols"] = statistics?.repairSymbols
                    report["final_transfer_statistics"] = finalRate
                    report["passed"] = self.liveTestPayload?.data == expected && finalRate && repaired
                    do {
                        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
                            .write(to: reportURL, options: .atomic)
                    } catch { self.status = error.localizedDescription }
                }
            } catch {
                self.stop()
                report["error"] = error.localizedDescription
                try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
                    .write(to: reportURL, options: .atomic)
            }
        }
    }

    func liveStreamSelfTest() {
        _ = beginImage("Checking the live BGRA stream path…")
        liveTestSuppressPayload = true
        liveTestPayload = nil
        decodeQueue.async { [weak self] in
            guard let self else { return }
            let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            let reportURL = directory.appendingPathComponent("live-stream-selftest-result.json")
            var report: [String: Any] = ["passed": false, "exact": false,
                "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
                "capture_source": "saved_bgra_provider", "synthetic_sensor_timestamps": true,
                "hardware_preview_verified": false, "hardware_frame_rate_verified": false,
                "live_4k_resolution_verified": false]
            func saveReport() {
                do {
                    try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
                        .write(to: reportURL, options: .atomic)
                } catch { self.recordCameraEvent("stream_selftest_report_error", fields: ["error": error.localizedDescription]) }
            }
            do {
                let preparationStarted = CACurrentMediaTime()
                let manifestData = try Data(contentsOf: directory.appendingPathComponent("recovery-regression.json"))
                guard let manifest = try JSONSerialization.jsonObject(with: manifestData) as? [String: Any],
                      let paths = manifest["frames"] as? [String], paths.count == 4,
                      let expectedPath = manifest["expected_payload"] as? String else {
                    throw CWFailure(message: "Live stream regression requires four camera images and an expected payload filename.")
                }
                let baseTimestamp = CACurrentMediaTime()
                let samples = try paths.enumerated().map { index, path in
                    try autoreleasepool {
                        try Codec.savedImageSampleBuffer(Data(contentsOf: Self.recoveryFixtureURL(path, directory: directory)),
                            timestamp: CMTime(seconds: baseTimestamp + Double(index) / 30, preferredTimescale: 600_000))
                    }
                }
                let clean = try Codec.savedImageSampleBuffer(Codec.encode(Codec.sample, cellPixels: 4).png,
                    timestamp: CMTime(seconds: baseTimestamp - 1, preferredTimescale: 600_000))
                report["load_and_conversion_ms"] = (CACurrentMediaTime() - preparationStarted) * 1000
                report["source_images"] = paths
                let output = AVCaptureVideoDataOutput()
                func run(_ sample: CMSampleBuffer) -> Double {
                    let started = CACurrentMediaTime()
                    self.captureOutput(output, didOutput: sample)
                    return (CACurrentMediaTime() - started) * 1000
                }
                self.state.lock.withLock { self.state.cameraWanted = true; self.state.automaticDecoding = true }
                report["clean_callback_ms"] = run(clean)
                DispatchQueue.main.async {
                    let cleanSucceeded = self.liveTestPayload?.data == Codec.sample && !self.state.lock.withLock { self.state.cameraWanted }
                    report["clean_first_frame_succeeded"] = cleanSucceeded
                    self.liveTestPayload = nil
                    self.state.lock.withLock { self.state.liveTestResultTiming = nil }
                    self.decodeQueue.async {
                        self.state.lock.withLock { self.state.cameraWanted = true }
                        report["cancellation_setup_callback_ms"] = run(samples[0])
                        let retainedBeforeCancel = self.decoding.recoverySession != nil && !(self.decoding.recoverySession?.captureIDs ?? []).isEmpty && !self.decoding.liveFailureFrames.isEmpty
                        DispatchQueue.main.async {
                            self.stop()
                            self.decodeQueue.async {
                                let cleared = self.decoding.recoverySession == nil && (self.decoding.recoverySession?.captureIDs ?? []).isEmpty && self.decoding.liveFailureFrames.isEmpty &&
                                    self.decoding.liveRecoveryStartedAt == 0 && self.decoding.firstLiveDetectedSampleAt == nil &&
                                    !self.state.lock.withLock { self.state.liveRecoveryActive }
                                _ = run(samples[0])
                                let ignoredAfterStop = self.decoding.recoverySession == nil && self.decoding.liveFailureFrames.isEmpty &&
                                    self.decoding.firstLiveDetectedSampleAt == nil
                                report["cancellation_cleared_retained_evidence"] = retainedBeforeCancel && cleared && ignoredAfterStop
                                report["cancellation_cleared_detection_timing"] = cleared && ignoredAfterStop
                                self.state.lock.withLock { self.state.cameraWanted = true }
                                let failureCallbacks = (0..<3).map { _ in run(samples[0]) }
                                report["duplicate_failure_callback_ms"] = failureCallbacks
                                let failureTerminated = self.decoding.recoverySession == nil && self.decoding.liveFailureFrames.isEmpty &&
                                    !self.state.lock.withLock { self.state.liveRecoveryActive }
                                report["duplicate_failure_terminated"] = failureTerminated
                                let firstDetectionAfterFailure = self.decoding.firstLiveDetectedSampleAt
                                let retryCallback = run(samples[0])
                                let immediateRetryStarted = self.decoding.liveRecoveryStartedAt > 0 && self.decoding.liveRecoveryAttempts == 1 &&
                                    self.state.lock.withLock { self.state.liveRecoveryActive } && firstDetectionAfterFailure != nil &&
                                    self.decoding.firstLiveDetectedSampleAt == firstDetectionAfterFailure
                                report["immediate_retry_callback_ms"] = retryCallback
                                report["immediate_retry_started_after_failed_run"] = immediateRetryStarted
                                report["first_detection_retained_across_failed_run"] = firstDetectionAfterFailure != nil &&
                                    self.decoding.firstLiveDetectedSampleAt == firstDetectionAfterFailure
                                self.decoding.failureArchiveQueue.async {
                                    var archiveVerified = false
                                    do {
                                        let latest = directory.appendingPathComponent("LiveFailures/latest", isDirectory: true)
                                        let data = try Data(contentsOf: latest.appendingPathComponent("diagnostics.json"))
                                        guard let archive = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                                              archive["capture_source"] as? String == "saved_bgra_provider",
                                              let frames = archive["frames"] as? [[String: Any]], !frames.isEmpty, frames.count <= 4,
                                              let pixels = CMSampleBufferGetImageBuffer(samples[0]) else {
                                            throw CWFailure(message: "No bounded saved-BGRA failure archive was produced.")
                                        }
                                        let width = CVPixelBufferGetWidth(pixels), height = CVPixelBufferGetHeight(pixels)
                                        let stride = CVPixelBufferGetBytesPerRow(pixels)
                                        archiveVerified = try frames.allSatisfy { frame in
                                            guard let filename = frame["filename"] as? String else { return false }
                                            let bytes = try latest.appendingPathComponent(filename).resourceValues(forKeys: [.fileSizeKey]).fileSize
                                            return frame["width"] as? Int == width && frame["height"] as? Int == height &&
                                                frame["stride"] as? Int == stride && frame["byte_length"] as? Int == stride * height && bytes == stride * height
                                        }
                                        report["failure_archive_frame_count"] = frames.count
                                        report["failure_archive_width"] = width
                                        report["failure_archive_height"] = height
                                        report["failure_archive_stride"] = stride
                                    } catch { report["failure_archive_error"] = error.localizedDescription }
                                    report["bounded_failure_archive_verified"] = archiveVerified
                                    DispatchQueue.main.async {
                                        self.stop()
                                        self.liveTestPayload = nil
                                        self.decodeQueue.async {
                                            self.state.lock.withLock { self.state.cameraWanted = true }
                                            let fixtureDetectionTimingReset = self.decoding.firstLiveDetectedSampleAt == nil
                                            let firstEligible = CACurrentMediaTime()
                                            var callbacks = [Double]()
                                            for sample in samples {
                                                guard self.state.lock.withLock({ self.state.cameraWanted }) else { break }
                                                callbacks.append(run(sample))
                                            }
                                            report["fixture_callback_ms"] = callbacks
                                            report["fixture_callback_sum_ms"] = callbacks.reduce(0, +)
                                            report["fixture_callbacks_total_elapsed_ms"] = (CACurrentMediaTime() - firstEligible) * 1000
                                            report["fixture_callback_count"] = callbacks.count
                                            DispatchQueue.main.async {
                                                let recovered = self.liveTestPayload
                                                let expected = recovered == nil ? nil : try? Data(contentsOf: Self.recoveryFixtureURL(expectedPath, directory: directory))
                                                let exact = recovered != nil && expected != nil && recovered?.data == expected
                                                let stopped = !self.state.lock.withLock { self.state.cameraWanted }
                                                let liveTiming = self.state.lock.withLock { self.state.liveTestResultTiming }
                                                let timingOrdered = liveTiming.map {
                                                    $0.decodedAt >= $0.firstDetectedAt && $0.resultAssignedAt >= $0.decodedAt
                                                } ?? false
                                                report["exact"] = exact
                                                report["stopped_after_success"] = stopped
                                                report["fixture_detection_timing_reset_before_start"] = fixtureDetectionTimingReset
                                                report["fixture_detection_to_payload_assignment_ms"] = liveTiming.map {
                                                    ($0.resultAssignedAt - $0.firstDetectedAt) * 1000
                                                }
                                                report["fixture_decode_to_payload_assignment_ms"] = liveTiming.map {
                                                    ($0.resultAssignedAt - $0.decodedAt) * 1000
                                                }
                                                report["fixture_timing_ordered"] = timingOrdered
                                                report["passed"] = cleanSucceeded && retainedBeforeCancel && cleared && ignoredAfterStop &&
                                                    failureTerminated && immediateRetryStarted && archiveVerified &&
                                                    fixtureDetectionTimingReset && timingOrdered && exact && stopped
                                                self.sessionQueue.async {
                                                    self.diagnostics.export { snapshot in
                                                        if let data = try? snapshot.get(),
                                                           let diagnostics = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                                                           let events = diagnostics["events"] as? [[String: Any]],
                                                           let success = events.last(where: { $0["event"] as? String == "decode_success" && $0["capture_source"] as? String == "saved_bgra_provider" }) {
                                                            report["first_detected_sample_to_decode_success_ms"] = success["first_detected_sample_to_decode_success_ms"]
                                                            report["rust_final_call_ms"] = success["rust_final_call_ms"]
                                                            report["first_frame_sensor_timestamp_seconds"] = success["first_frame_sensor_timestamp_seconds"]
                                                            report["last_frame_sensor_timestamp_seconds"] = success["frame_timestamp_seconds"]
                                                        }
                                                        saveReport()
                                                        self.liveTestSuppressPayload = false
                                                        self.processingImage = false
                                                        self.status = report["passed"] as? Bool == true ? "Live stream self-test passed" : "Live stream self-test failed"
                                                        self.diagnostic = "Saved BGRA samples exercised the live processing path. Physical acquisition and 4K detail were not tested."
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } catch {
                report["error"] = error.localizedDescription
                saveReport()
                let ticket = self.state.lock.withLock { self.state.generation }
                self.failed(error, ticket: ticket)
            }
        }
    }

    func recoveryCameraSelfTest() {
        let ticket = beginImage("Combining recovery regression photos…")
        decodeQueue.async { [weak self] in
            guard let self else { return }
            let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            let output = directory.appendingPathComponent("recovery-regression-result.json")
            let started = CACurrentMediaTime()
            var session: RecoverySession?
            var report: [String: Any] = ["exact": false,
                "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown"]
            var frameReports = [[String: Any]]()
            var recovered: DecodedPayload?
            var failure: String?
            do {
                let manifestData = try Data(contentsOf: directory.appendingPathComponent("recovery-regression.json"))
                guard let manifest = try JSONSerialization.jsonObject(with: manifestData) as? [String: Any],
                      let framePaths = manifest["frames"] as? [String], !framePaths.isEmpty, framePaths.count <= 4,
                      let expectedFilename = manifest["expected_payload"] as? String else {
                    throw CWFailure(message: "Recovery regression manifest must contain one to four frame paths and an expected payload filename.")
                }
                report["frame_count"] = framePaths.count
                report["max_frames"] = 4
                for relativePath in framePaths {
                    let frameURL = try Self.recoveryFixtureURL(relativePath, directory: directory)
                    let imageData = try Data(contentsOf: frameURL)
                    let captureID = UUID()
                    let (frame, fields) = try Codec.scanPhoto(imageData, captureID: captureID) { pixels, length, width, height, stride in
                        if session == nil { session = try RecoverySession() }
                        return try session!.decode(pixels: pixels, length: length, width: width, height: height,
                            stride: stride, captureID: captureID)
                    }
                    guard session != nil else { throw CWFailure(message: "Cannot create a recovery session.") }
                    var frameReport: [String: Any] = ["capture_id": captureID.uuidString.lowercased(),
                        "source": relativePath,
                        "width": frame.width, "height": frame.height, "resize_applied": fields["resize_applied"] ?? false,
                        "recovery_status": frame.recoveryStatus ?? "single_photo_decoded"]
                    if let frames = frame.recoveryFrames { frameReport["frames"] = frames }
                    if let certified = frame.recoveryCertifiedBlocks { frameReport["certified_blocks"] = certified }
                    if let total = frame.recoveryTotalBlocks { frameReport["total_blocks"] = total }
                    if let error = frame.recoveryMessage { frameReport["message"] = error }
                    frameReports.append(frameReport)
                    if let decoded = frame.decodedPayload {
                        guard frame.recoveryStatus == "decoded", let frames = frame.recoveryFrames, frames > 1 else {
                            throw CWFailure(message: "A regression photo decoded without combining multiple views.")
                        }
                        recovered = decoded
                        break
                    }
                    guard frame.recoveryStatus == "pending" || frame.recoveryStatus == "error" else {
                        throw CWFailure(message: frame.decodeError ?? "Recovery returned no status.")
                    }
                }
                guard let recovered else { throw CWFailure(message: "The supplied photos did not recover a payload.") }
                let expectedURL = try Self.recoveryFixtureURL(expectedFilename, directory: directory)
                let expected = try Data(contentsOf: expectedURL)
                report["recovered_bytes"] = recovered.data.count
                report["expected_bytes"] = expected.count
                report["profile"] = recovered.profile
                report["rust_final_call_ms"] = recovered.finalCallMilliseconds
                report["corrected"] = recovered.corrected
                report["erased"] = recovered.erased
                guard recovered.data == expected else { throw CWFailure(message: "Recovery regression payload mismatch.") }
                let content = PayloadContent(data: recovered.data)
                guard content.isGIF, content.gifFrameCount > 0 else { throw CWFailure(message: "Recovered payload is not a readable GIF.") }
                let gifURL = directory.appendingPathComponent("recovered.gif")
                try recovered.data.write(to: gifURL, options: .atomic)
                guard try Data(contentsOf: gifURL) == expected else { throw CWFailure(message: "Recovered GIF export changed the payload bytes.") }
                report["gif_frames"] = content.gifFrameCount
                report["recovered_gif"] = gifURL.lastPathComponent
                report["export_exact"] = true
                report["exact"] = true
            } catch {
                failure = error.localizedDescription
                report["error"] = error.localizedDescription
            }
            report["frames"] = frameReports
            report["frames_used"] = frameReports.count
            report["capture_ids"] = session?.captureIDs.map { $0.uuidString.lowercased() } ?? []
            if let session { report["recovery_session_id"] = session.id.uuidString.lowercased() }
            report["elapsed_milliseconds"] = (CACurrentMediaTime() - started) * 1000
            do {
                try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted])
                    .write(to: output, options: .atomic)
            } catch {
                self.failed(CWFailure(message: "Cannot save recovery regression result: \(error.localizedDescription)"), ticket: ticket)
                return
            }
            DispatchQueue.main.async {
                guard self.state.isCurrent(ticket) else { return }
                self.processingImage = false
                self.status = failure == nil ? "Multi-photo recovery self-test passed" : "Multi-photo recovery self-test failed"
                self.diagnostic = failure ?? "Combined \(frameReports.count) still photos and exported the exact GIF."
            }
        }
    }

    static func recoveryFixtureURL(_ relativePath: String, directory: URL) throws -> URL {
        guard !relativePath.isEmpty, !relativePath.hasPrefix("/") else {
            throw CWFailure(message: "Recovery fixture paths must be relative to Documents.")
        }
        let root = directory.standardizedFileURL.path + "/"
        let url = directory.appendingPathComponent(relativePath).standardizedFileURL
        guard url.path.hasPrefix(root) else { throw CWFailure(message: "Recovery fixture paths must stay inside Documents.") }
        return url
    }
}

extension Codec {
    static func savedImageSampleBuffer(_ data: Data, timestamp: CMTime) throws -> CMSampleBuffer {
        let (sample, _) = try withPhotoPixels(data) { buffer, width, height, stride, _, _ in
            var pixelBuffer: CVPixelBuffer?
            guard CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA,
                nil, &pixelBuffer) == kCVReturnSuccess, let pixelBuffer else {
                throw CWFailure(message: "Cannot prepare saved BGRA pixels.")
            }
            CVPixelBufferLockBaseAddress(pixelBuffer, [])
            defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, []) }
            guard let destination = CVPixelBufferGetBaseAddress(pixelBuffer) else {
                throw CWFailure(message: "Cannot access saved BGRA pixels.")
            }
            let destinationStride = CVPixelBufferGetBytesPerRow(pixelBuffer)
            for row in 0..<height {
                memcpy(destination.advanced(by: row * destinationStride), buffer.baseAddress!.advanced(by: row * stride), stride)
            }
            var format: CMVideoFormatDescription?
            guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault,
                imageBuffer: pixelBuffer, formatDescriptionOut: &format) == noErr, let format else {
                throw CWFailure(message: "Cannot describe saved BGRA pixels.")
            }
            var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 30),
                presentationTimeStamp: timestamp, decodeTimeStamp: .invalid)
            var sample: CMSampleBuffer?
            guard CMSampleBufferCreateReadyWithImageBuffer(allocator: kCFAllocatorDefault,
                imageBuffer: pixelBuffer, formatDescription: format, sampleTiming: &timing,
                sampleBufferOut: &sample) == noErr, let sample else {
                throw CWFailure(message: "Cannot prepare the saved BGRA sample.")
            }
            return sample
        }
        return sample
    }

}
#endif
