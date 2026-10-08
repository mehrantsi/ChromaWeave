import AVFoundation
import Combine
import UIKit
import ImageIO

enum CameraConfiguration { case camera, savedFrames }

enum ScanMode: String, CaseIterable, Identifiable {
    case live = "Live", photo = "Photo", transfer = "Transfer"
    var id: String { rawValue }
}

enum PhotoCamera: String, CaseIterable, Identifiable {
    case automatic, main
    var id: String { rawValue }
    var title: String { self == .automatic ? "Automatic" : "Main camera" }
}

final class CameraScanner: NSObject, ObservableObject {
    let state: CameraScanState
    lazy var acquisition: CameraAcquisition = CameraAcquisition(state: state, diagnostics: diagnostics,
        output: { [weak self] in self?.receive($0) },
        frameHandler: { [weak self] in self?.decoding.enqueueVideoFrame($0) },
        droppedFrameHandler: { [weak self] in self?.decoding.recordDroppedFrame() })
    lazy var decoding: CameraDecodeCoordinator = CameraDecodeCoordinator(state: state, acquisition: acquisition,
        output: { [weak self] in self?.receive($0) })

    var session: AVCaptureSession { acquisition.session }
    @Published var result: DecodedPayload?
    @Published var status = "Camera ready"
    @Published var diagnostic = "Keep the complete code and all four corner markers in view."
    @Published var focusStatus = "Continuous focus"
    @Published var cameraAvailable = true
    @Published var processingImage = false
    @Published var liveCamera = false
    @Published var photoResolution = ""
    @Published var photoAvailable = false
    @Published var photoCamera = PhotoCamera.automatic
    @Published var mainCameraAvailable = false
    @Published var photoFocusGuidance = ""
    @Published var transferProgress: Double?
    @Published var transferSummary = ""
    var transferRateMeter: TransferRateMeter?
    @Published var capturedFrame: CapturedFrame?
    @Published var lastCapturedDiagnostics: (id: UUID, result: Result<Data, Error>)?
    var diagnosticsExportTitle: String { lastCapturedDiagnostics == nil ? "Export diagnostics" : "Export last captured-frame diagnostics" }
    var diagnosticsFilename: String {
        if let capture = lastCapturedDiagnostics { return "cw-camera-frame-\(capture.id.uuidString.lowercased())-diagnostics.json" }
        return "cw-scan-diagnostics.json"
    }
    var sessionQueue: DispatchQueue { acquisition.sessionQueue }
    var captureQueue: DispatchQueue { acquisition.captureQueue }
    var decodeQueue: DispatchQueue { decoding.decodeQueue }
    #if CW_DEVELOPMENT
    var liveTestSuppressPayload = false
    var liveTestPayload: DecodedPayload?
    #endif
    let diagnostics = ScanDiagnosticStore()

    init(configuration: CameraConfiguration = .camera) {
        state = CameraScanState(configuration: configuration)
        super.init()
        _ = acquisition
        _ = decoding
    }

    private func receive(_ output: CameraScanOutput) {
        switch output {
        case .invalidated:
            decoding.resetPhotoRecovery()
        case let .failure(error, ticket):
            failed(error, ticket: ticket)
        case let .accepted(payload, ticket, timing):
            accept(payload, ticket: ticket, liveTiming: timing)
        case let .photo(outcome, metadata, captureID, ticket):
            decoding.scanPhoto(outcome, metadata: metadata, captureID: captureID, ticket: ticket)
        case let .presentation(presentation, ticket):
            guard state.isCurrent(ticket) else { return }
            if let value = presentation.status { status = value }
            if let value = presentation.diagnostic { diagnostic = value }
            if let value = presentation.focusStatus { focusStatus = value }
            if let value = presentation.cameraAvailable { cameraAvailable = value }
            if let value = presentation.processingImage { processingImage = value }
            if let value = presentation.liveCamera { liveCamera = value }
            if let value = presentation.photoResolution { photoResolution = value }
            if let value = presentation.photoAvailable { photoAvailable = value }
            if let value = presentation.mainCameraAvailable { mainCameraAvailable = value }
            if let value = presentation.photoFocusGuidance { photoFocusGuidance = value }
        case let .captured(frame, diagnostics, ticket, savedPhoto):
            guard state.isCurrent(ticket) else { return }
            state.lock.withLock {
                if savedPhoto { state.photoInFlight = false }
                state.generation += 1
            }
            if savedPhoto { processingImage = false }
            else { liveCamera = false }
            lastCapturedDiagnostics = (frame.id, diagnostics)
            capturedFrame = frame
            status = savedPhoto
                ? (frame.decodedPayload == nil ? "Photo captured; no payload recovered" : "Photo captured; payload recovered")
                : "Frame and diagnostics captured"
        case let .transfer(update, startedAt, validatedAt, updateUI, ticket):
            guard state.isCurrent(ticket) else { return }
            if transferRateMeter == nil { transferRateMeter = TransferRateMeter(startedAt: startedAt) }
            transferRateMeter?.record(totalBytes: update.uniqueValidatedBytes, at: validatedAt)
            guard updateUI else { return }
            let complete = update.completedData != nil
            let elapsed = validatedAt - startedAt
            let megabits = Double(update.uniqueValidatedBytes) * 8 / max(elapsed, 0.000001) / 1_000_000
            transferProgress = Double(update.receivedChunks) / Double(update.totalChunks)
            transferSummary = complete
                ? String(format: "SHA-256 verified · %.2f Mbit/s · %.2f s", megabits, elapsed)
                : "\(update.receivedChunks) of \(update.totalChunks) chunks saved · \(update.uniqueValidatedBytes.formatted()) bytes"
            if !complete && update.repairSymbols > 0 { transferSummary += " · \(update.repairedChunks) repaired" }
            status = complete ? "Transfer verified" : "Receiving CW transfer"
        case let .transferFailure(detected, message, ticket):
            guard state.isCurrent(ticket) else { return }
            status = detected ? "CW detected · Retry needed" : (transferProgress == nil ? "Looking for CW transfer" : "Receiving CW transfer")
            diagnostic = message
        case let .resetTransfer(ticket):
            guard state.isCurrent(ticket) else { return }
            transferProgress = nil
            transferSummary = ""
            transferRateMeter = nil
        }
    }

    func recordCameraEvent(_ event: String, fields: [String: Any] = [:]) {
        acquisition.recordCameraEvent(event, fields: fields)
    }

    func focus(at point: CGPoint) { acquisition.focus(at: point) }
    func refocus() { acquisition.refocus() }

    func setPhotoCamera(_ selection: PhotoCamera) {
        guard selection != .main || mainCameraAvailable,
              let reconfigure = acquisition.selectPhotoCamera(selection) else { return }
        decoding.resetPhotoRecovery()
        photoCamera = selection
        guard reconfigure else { return }
        liveCamera = false
        processingImage = true
        status = "Configuring still camera…"
        acquisition.configureAndStart()
    }

    func scanPhoto() {
        guard liveCamera, photoAvailable else { return }
        acquisition.scanPhoto()
    }

    func exportDiagnostics(completion: @escaping (Result<Data, Error>) -> Void) {
        if let capture = lastCapturedDiagnostics {
            DispatchQueue.main.async { completion(capture.result) }
            return
        }
        sessionQueue.async { [weak self] in self?.diagnostics.export(completion: completion) }
    }

    func start() {
        guard result == nil, capturedFrame == nil, !processingImage else { return }
        let newScan = state.lock.withLock { () -> Bool in
            guard !state.cameraWanted else { return false }
            state.cameraWanted = true
            state.generation += 1
            return true
        }
        if newScan { decoding.resetPhotoRecovery() }
        #if targetEnvironment(simulator)
        acquisition.unavailable("No camera is available in Simulator. Import an image or try the sample.")
        #else
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: acquisition.configureAndStart()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
                if granted { self?.acquisition.configureAndStart() }
                else { self?.acquisition.unavailable("Camera access is off. You can still import a CW image.") }
            }
        default: acquisition.unavailable("Allow camera access in Settings, or import a CW image.")
        }
        #endif
    }

    func stop() {
        state.lock.withLock { state.cameraWanted = false; state.captureRequested = false; state.photoInFlight = false; state.cameraConfiguring = false; state.generation += 1 }
        decoding.resetPhotoRecovery()
        liveCamera = false
        processingImage = false
        acquisition.stopSession()
    }

    func resume() {
        result = nil
        capturedFrame = nil
        status = state.scanningStatus
        start()
    }

    var scanningStatus: String { state.scanningStatus }

    func setScanMode(_ mode: ScanMode) {
        let enabled = mode == .photo
        let transfer = mode == .transfer
        let (changed, reconfigure) = state.lock.withLock { () -> (Bool, Bool) in
            guard state.automaticDecoding == enabled || state.transferDecoding != transfer else { return (false, false) }
            state.automaticDecoding = !enabled
            state.transferDecoding = transfer
            state.liveRecoveryActive = false
            state.captureRequested = false
            state.generation += 1
            state.cameraConfiguring = state.cameraWanted
            return (true, state.cameraWanted)
        }
        guard changed else { return }
        if transfer { lastCapturedDiagnostics = nil }
        decoding.resetPhotoRecovery()
        guard reconfigure else { return }
        liveCamera = false
        processingImage = true
        status = enabled ? "Configuring still camera…" : "Configuring live camera…"
        acquisition.configureAndStart()
    }

    func restartTransfer() {
        stop()
        resume()
    }

    func liveTransferMbps(at time: CFTimeInterval) -> Double? {
        transferRateMeter?.liveMbps(at: time)
    }

    func captureFrame() {
        guard liveCamera else { return }
        state.lock.withLock { state.captureRequested = true }
        status = "Capturing next camera frame…"
    }

    func isCurrent(_ ticket: Int) -> Bool { state.isCurrent(ticket) }

    func accept(_ decoded: DecodedPayload, ticket: Int, liveTiming: LiveDecodeTiming? = nil) {
        decoding.clearRecovery()
        guard state.lock.withLock({ () -> Bool in
            guard state.generation == ticket else { return false }
            state.cameraWanted = false
            state.photoInFlight = false
            state.liveRecoveryActive = false
            return true
        }) else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.state.isCurrent(ticket) else { return }
            let resultGeneration = self.state.lock.withLock { () -> Int? in
                guard self.state.generation == ticket else { return nil }
                self.state.cameraWanted = false
                self.state.generation += 1
                return self.state.generation
            }
            guard let resultGeneration else { return }
            self.processingImage = false
            self.liveCamera = false
            var assignedPayload = decoded
            if let liveTiming {
                assignedPayload.scanMilliseconds = (CACurrentMediaTime() - liveTiming.firstDetectedAt) * 1000
            }
            var suppressPresentation = false
            #if CW_DEVELOPMENT
            suppressPresentation = self.liveTestSuppressPayload
            if self.liveTestSuppressPayload {
                self.liveTestPayload = assignedPayload
                if let liveTiming {
                    let testTiming = LiveSheetTiming(payloadID: decoded.id, generation: resultGeneration,
                        firstDetectedAt: liveTiming.firstDetectedAt, decodedAt: liveTiming.decodedAt,
                        resultAssignedAt: CACurrentMediaTime(), captureSource: liveTiming.captureSource,
                        attemptID: liveTiming.attemptID)
                    self.state.lock.withLock { self.state.liveTestResultTiming = testTiming; self.state.pendingLiveSheetTiming = nil }
                } else {
                    self.state.lock.withLock { self.state.liveTestResultTiming = nil; self.state.pendingLiveSheetTiming = nil }
                }
            }
            #endif
            if !suppressPresentation {
                self.result = assignedPayload
                let resultAssignedAt = CACurrentMediaTime()
                if let liveTiming {
                    let sheetTiming = LiveSheetTiming(payloadID: decoded.id, generation: resultGeneration,
                        firstDetectedAt: liveTiming.firstDetectedAt, decodedAt: liveTiming.decodedAt,
                        resultAssignedAt: resultAssignedAt, captureSource: liveTiming.captureSource,
                        attemptID: liveTiming.attemptID)
                    self.state.lock.withLock { self.state.pendingLiveSheetTiming = sheetTiming }
                    self.recordCameraEvent("result_assigned", fields: [
                        "capture_source": liveTiming.captureSource,
                        "attempt_id": liveTiming.attemptID,
                        "payload_id": decoded.id.uuidString.lowercased(),
                        "first_detected_sample_received_at_monotonic_seconds": liveTiming.firstDetectedAt,
                        "decode_success_at_monotonic_seconds": liveTiming.decodedAt,
                        "result_assigned_at_monotonic_seconds": resultAssignedAt,
                        "first_detected_sample_to_result_assignment_ms": (resultAssignedAt - liveTiming.firstDetectedAt) * 1000,
                        "decode_success_to_result_assignment_ms": (resultAssignedAt - liveTiming.decodedAt) * 1000,
                        "payload_length": decoded.data.count
                    ])
                } else {
                    self.state.lock.withLock { self.state.pendingLiveSheetTiming = nil }
                }
            }
            self.status = "Payload recovered"
            self.acquisition.stopSession()
        }
    }

    func payloadSheetAppeared(for payload: DecodedPayload) {
        let appearedAt = CACurrentMediaTime()
        let timing = state.lock.withLock { () -> LiveSheetTiming? in
            guard let timing = state.pendingLiveSheetTiming, timing.payloadID == payload.id,
                  timing.generation == state.generation, result?.id == payload.id else { return nil }
            state.pendingLiveSheetTiming = nil
            return timing
        }
        guard let timing else { return }
        recordCameraEvent("payload_sheet_on_appear", fields: [
            "capture_source": timing.captureSource,
            "attempt_id": timing.attemptID,
            "payload_id": payload.id.uuidString.lowercased(),
            "first_detected_sample_received_at_monotonic_seconds": timing.firstDetectedAt,
            "decode_success_at_monotonic_seconds": timing.decodedAt,
            "result_assigned_at_monotonic_seconds": timing.resultAssignedAt,
            "payload_sheet_on_appear_at_monotonic_seconds": appearedAt,
            "first_detected_sample_to_payload_sheet_on_appear_ms": (appearedAt - timing.firstDetectedAt) * 1000,
            "result_assignment_to_payload_sheet_on_appear_ms": (appearedAt - timing.resultAssignedAt) * 1000
        ])
    }

    func beginImage(_ message: String) -> Int {
        let ticket = state.lock.withLock { state.cameraWanted = false; state.captureRequested = false; state.photoInFlight = false; state.cameraConfiguring = false; state.generation += 1; return state.generation }
        decoding.resetPhotoRecovery()
        acquisition.stopSession()
        processingImage = true
        liveCamera = false
        status = message
        return ticket
    }

    func failed(_ error: Error, ticket: Int) {
        guard state.isCurrent(ticket) else { return }
        if !state.lock.withLock({ state.cameraWanted }) { decoding.resetPhotoRecovery(force: true) }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.state.isCurrent(ticket) else { return }
            self.processingImage = false
            self.status = "No payload recovered"
            self.diagnostic = error.localizedDescription
        }
    }

    func importImage(_ image: UIImage) {
        let ticket = beginImage("Reading image…")
        decodeQueue.async { [weak self] in
            do { self?.accept(try Codec.decode(image: image), ticket: ticket) }
            catch { self?.failed(error, ticket: ticket) }
        }
    }

    func importFailed() {
        status = "Cannot read that image"
        diagnostic = "Choose another image from Photos."
    }


    func sample(_ kind: CodeSample = .binary, profile: Int = Codec.defaultProfile) {
        let ticket = beginImage("Reading \(kind.title.lowercased())…")
        decodeQueue.async { [weak self] in
            do {
                let encoded = try Codec.encode(kind.data, profile: profile, cellPixels: 2)
                let decoded = try Codec.decode(image: encoded.image)
                guard decoded.data == kind.data else { throw CWFailure(message: "Sample payload mismatch.") }
                self?.accept(decoded, ticket: ticket)
            } catch { self?.failed(error, ticket: ticket) }
        }
    }
}
