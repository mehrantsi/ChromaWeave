import AVFoundation
import UIKit

final class CameraDecodeCoordinator {
    let decodeQueue = DispatchQueue(label: "cw.camera.decode", qos: .userInitiated)
    private(set) var decodeStateGeneration: Int?
    private(set) var transferReceiver = TransferReceiver(repair: Codec.repairTransfer)
    private(set) var transferScannerSession: TransferScannerSession?
    private(set) var transferScannerLensRevision: Int?
    private(set) var transferStartedAt: CFTimeInterval?
    private(set) var transferFirstAttemptAt: CFTimeInterval?
    private(set) var transferAttempts = 0
    private(set) var transferDecodeFailures = 0
    private(set) var transferPacketFailures = 0
    private(set) var transferSavedFailure = false
    private(set) var transferSavedSuccess = false
    private(set) var liveSavedSuccess = false
    private(set) var lastTransferUIUpdateAt = 0.0
    private(set) var lastTransferFailureLogAt = 0.0
    private(set) var liveRecoveryStartedAt = 0.0
    private(set) var liveRecoveryAttempts = 0
    private(set) var liveRecoveryRejections = 0
    private(set) var firstLiveDetectedSampleAt: CFTimeInterval?
    private(set) var liveFailureFrames = [LiveFailureFrame]()
    private(set) var firstLiveSensorTimestamp = 0.0
    private(set) var previousLiveSensorTimestamp = 0.0
    private(set) var recoverySession: RecoverySession?
    private let state: CameraScanState
    private let acquisition: CameraAcquisition
    private let output: (CameraScanOutput) -> Void
    private(set) var pendingVideoFrames = [CameraVideoFrame]()
    private(set) var videoWorkerScheduled = false
    private(set) var videoBuffersReceived = 0
    private(set) var videoBuffersOverwritten = 0
    private(set) var videoBuffersDropped = 0
    let failureArchiveQueue = DispatchQueue(label: "cw.camera.failure-archive", qos: .utility)
    private(set) var failureArchiveBusy = false
    private(set) var nextFailureArchiveAt = 0.0

    init(state: CameraScanState, acquisition: CameraAcquisition, output: @escaping (CameraScanOutput) -> Void) {
        self.state = state
        self.acquisition = acquisition
        self.output = output
    }

    func recordDroppedFrame() {
        state.lock.withLock {
            if state.cameraWanted && !state.cameraConfiguring && state.automaticDecoding { videoBuffersDropped += 1 }
        }
    }

    func clearRecovery() {
        dispatchPrecondition(condition: .onQueue(decodeQueue))
        recoverySession = nil
    }

    func recover(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int,
                 captureID: UUID) throws -> PhotoRecoveryResult {
        dispatchPrecondition(condition: .onQueue(decodeQueue))
        if recoverySession == nil { recoverySession = try RecoverySession() }
        return try recoverySession!.decode(pixels: pixels, length: length, width: width, height: height,
            stride: stride, captureID: captureID)
    }

    func reset(for ticket: Int) {
        dispatchPrecondition(condition: .onQueue(decodeQueue))
        guard decodeStateGeneration != ticket else { return }
        decodeStateGeneration = ticket
        transferReceiver.reset()
        transferScannerSession?.reset()
        transferScannerLensRevision = nil
        transferStartedAt = nil
        transferFirstAttemptAt = nil
        transferAttempts = 0
        transferDecodeFailures = 0
        transferPacketFailures = 0
        transferSavedFailure = false
        transferSavedSuccess = false
        lastTransferUIUpdateAt = 0
        lastTransferFailureLogAt = 0
        clearRecovery()
        liveRecoveryStartedAt = 0
        liveRecoveryAttempts = 0
        liveRecoveryRejections = 0
        firstLiveDetectedSampleAt = nil
        liveFailureFrames.removeAll(keepingCapacity: false)
        firstLiveSensorTimestamp = 0
        previousLiveSensorTimestamp = 0
    }
}

extension CameraDecodeCoordinator {
    func scanPhoto(_ outcome: Result<(Data, [String: Any]), Error>, metadata fields: [String: Any],
                   captureID: UUID, ticket: Int) {
        decodeQueue.async { [self] in
            guard state.isCurrent(ticket) else { return }
            do {
                let (data, photoFields) = try outcome.get()
                let (frame, decodedFields) = try Codec.scanPhoto(data, captureID: captureID)
                var metadata = fields.merging(photoFields) { _, actual in actual }
                metadata.merge(decodedFields) { _, actual in actual }
                self.acquisition.recordCameraEvent("photo_scanned", fields: metadata)
                let snapshot = metadata
                self.acquisition.sessionQueue.async { [self] in
                    self.acquisition.diagnostics.export(capture: snapshot) { [weak self] result in
                        guard let self, self.state.isCurrent(ticket) else { return }
                        var frame = frame
                        frame.diagnostics = result
                        self.output(.presentation(CameraPresentation(status: "Saving captured photo…"), ticket: ticket))
                        let finalizedFrame = frame
                        self.decodeQueue.async {
                            var savedFrame = finalizedFrame
                            do {
                                try CapturedPhotoArchive.save(finalizedFrame)
                                savedFrame.savedOnDevice = true
                            } catch { savedFrame.saveError = error.localizedDescription }
                            let completedFrame = savedFrame
                            DispatchQueue.main.async {
                                guard self.state.isCurrent(ticket) else { return }
                                self.output(.captured(completedFrame, diagnostics: result, ticket: ticket, savedPhoto: true))
                            }
                        }
                    }
                }
            } catch {
                self.acquisition.recordCameraEvent("photo_error", fields: fields.merging(["error": error.localizedDescription]) { _, value in value })
                self.state.lock.withLock { if self.state.generation == ticket { self.state.photoInFlight = false } }
                self.output(.failure(error, ticket: ticket))
            }
        }
    }

    func resetPhotoRecovery(force: Bool = false) {
        let ticket = state.lock.withLock {
            state.liveRecoveryActive = false
            state.pendingLiveSheetTiming = nil
            #if CW_DEVELOPMENT
            state.liveTestResultTiming = nil
            #endif
            pendingVideoFrames.removeAll(keepingCapacity: true)
            videoBuffersReceived = 0
            videoBuffersOverwritten = 0
            videoBuffersDropped = 0
            return state.generation
        }
        decodeQueue.async { [self] in
            guard state.isCurrent(ticket) else { return }
            if force { decodeStateGeneration = nil }
            reset(for: ticket)
        }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.state.isCurrent(ticket) else { return }
            self.output(.resetTransfer(ticket: ticket))
        }
    }

    func enqueueVideoFrame(_ sampleBuffer: CMSampleBuffer) {
        guard let frame = videoFrame(sampleBuffer) else { return }
        let schedule = state.lock.withLock { () -> Bool in
            guard state.cameraWanted, state.generation == frame.generation else { return false }
            videoBuffersReceived += 1
            guard !pendingVideoFrames.contains(where: \.capture) else {
                videoBuffersOverwritten += 1
                return false
            }
            if frame.capture {
                videoBuffersOverwritten += pendingVideoFrames.count
                pendingVideoFrames.removeAll(keepingCapacity: true)
            }
            let limit = state.transferDecoding ? 2 : 1
            if pendingVideoFrames.count >= limit {
                pendingVideoFrames.removeFirst()
                videoBuffersOverwritten += 1
            }
            pendingVideoFrames.append(frame)
            guard !videoWorkerScheduled else { return false }
            videoWorkerScheduled = true
            return true
        }
        if schedule { decodeQueue.async { [weak self] in self?.drainVideoFrame() } }
    }

    func drainVideoFrame() {
        let frame = state.lock.withLock { () -> CameraVideoFrame? in
            guard !pendingVideoFrames.isEmpty else {
                videoWorkerScheduled = false
                return nil
            }
            return pendingVideoFrames.removeFirst()
        }
        guard let frame else { return }
        autoreleasepool { processVideoFrame(frame) }
        let schedule = state.lock.withLock { () -> Bool in
            guard !pendingVideoFrames.isEmpty else {
                videoWorkerScheduled = false
                return false
            }
            return true
        }
        if schedule { decodeQueue.async { [weak self] in self?.drainVideoFrame() } }
    }

    func videoFrame(_ sampleBuffer: CMSampleBuffer) -> CameraVideoFrame? {
        let receivedAt = CACurrentMediaTime()
        guard let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return nil }
        guard let request = state.lock.withLock({ () -> (Int, Bool, Int)? in
            guard state.cameraWanted, !state.photoInFlight, !state.cameraConfiguring, state.automaticDecoding || state.captureRequested else { return nil }
            let capture = state.captureRequested
            state.captureRequested = false
            return (state.generation, capture, state.lensRevision)
        }) else { return nil }
        return CameraVideoFrame(pixels: pixels, receivedAt: receivedAt,
            sensorTimestamp: CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sampleBuffer)),
            generation: request.0, capture: request.1, lensRevision: request.2)
    }

    func processVideoFrame(_ frame: CameraVideoFrame) {
        let ticket = frame.generation
        guard state.lock.withLock({ state.cameraWanted && state.generation == ticket && !state.photoInFlight && !state.cameraConfiguring }) else { return }
        reset(for: ticket)
        let now = frame.receivedAt
        let decodeStartedAt = CACurrentMediaTime()
        let capture = frame.capture
        let pixels = frame.pixels
        CVPixelBufferLockBaseAddress(pixels, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixels, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pixels) else { return }
        let width = CVPixelBufferGetWidth(pixels)
        let height = CVPixelBufferGetHeight(pixels)
        let stride = CVPixelBufferGetBytesPerRow(pixels)
        var frameFields: [String: Any] = ["width": width, "height": height, "stride": stride,
                                        "frame_timestamp_seconds": frame.sensorTimestamp,
                                        "queue_wait_ms": (decodeStartedAt - now) * 1000,
                                        "decode_started_at_unix_seconds": Date().timeIntervalSince1970,
                                        "lens_revision": frame.lensRevision,
                                        "decode_started_at_monotonic_seconds": decodeStartedAt,
                                        "pixel_buffer_color_attachments": [
                                            "primaries": CVBufferCopyAttachment(pixels, kCVImageBufferColorPrimariesKey, nil) as? String ?? "absent",
                                            "transfer_function": CVBufferCopyAttachment(pixels, kCVImageBufferTransferFunctionKey, nil) as? String ?? "absent",
                                            "ycbcr_matrix": CVBufferCopyAttachment(pixels, kCVImageBufferYCbCrMatrixKey, nil) as? String ?? "absent"
                                        ]]
        frameFields.merge(state.lock.withLock { [
            "camera_buffers_received": videoBuffersReceived,
            "camera_buffers_dropped": videoBuffersDropped,
            "pending_buffers_overwritten": videoBuffersOverwritten,
            "pending_buffers": pendingVideoFrames.count,
            "capture_thermal_state": state.thermalState
        ] }) { _, value in value }
        if capture {
            do {
                let frame = try Codec.frame(pixels: base, width: width, height: height, stride: stride)
                frameFields["capture_id"] = frame.id.uuidString.lowercased()
                frameFields["decoded"] = false
                guard state.lock.withLock({
                    guard state.generation == ticket else { return false }
                    state.cameraWanted = false
                    return true
                }) else { return }
                liveRecoveryStartedAt = 0
                liveFailureFrames.removeAll(keepingCapacity: false)
                clearRecovery()
                acquisition.recordCameraEvent("frame_captured", fields: frameFields)
                let captureMetadata = frameFields
                acquisition.sessionQueue.async { [weak self] in
                    guard let self else { return }
                    self.acquisition.diagnostics.export(capture: captureMetadata) { [weak self] result in
                        guard let self, self.state.isCurrent(ticket) else { return }
                        var frame = frame
                        frame.diagnostics = result
                        self.output(.captured(frame, diagnostics: result, ticket: ticket, savedPhoto: false))
                        self.acquisition.stopSession()
                    }
                }
            } catch { output(.failure(error, ticket: ticket)) }
            return
        }
        if state.lock.withLock({ state.transferDecoding }) {
            if transferScannerLensRevision != frame.lensRevision {
                transferScannerSession?.reset()
                transferScannerLensRevision = frame.lensRevision
            }
            receiveTransferFrame(pixels: base.assumingMemoryBound(to: UInt8.self), length: stride * height,
                width: width, height: height, stride: stride, receivedAt: now,
                decodeStartedAt: decodeStartedAt, frameFields: frameFields, ticket: ticket)
            return
        }
        if liveRecoveryStartedAt > 0, liveRecoveryAttempts >= 8 || now - liveRecoveryStartedAt >= 45 {
            finishLiveRecovery(ticket: ticket, message: "Keep the whole code in view and adjust its distance or angle slightly.")
            return
        }
        let captureID = UUID()
        frameFields["attempt_id"] = captureID.uuidString.lowercased()
        frameFields["capture_source"] = state.liveCaptureSource
        frameFields["sample_received_at_monotonic_seconds"] = now
        let sensorTimestamp = frameFields["frame_timestamp_seconds"] as? Double ?? 0
        if previousLiveSensorTimestamp > 0 { frameFields["sensor_frame_gap_ms"] = (sensorTimestamp - previousLiveSensorTimestamp) * 1000 }
        previousLiveSensorTimestamp = sensorTimestamp
        acquisition.recordCameraEvent("decode_started", fields: frameFields)
        do {
            let recovered = try recover(pixels: base.assumingMemoryBound(to: UInt8.self), length: stride * height,
                width: width, height: height, stride: stride, captureID: captureID)
            guard state.isCurrent(ticket) else { return }
            var fields = frameFields
            fields["decode_ms"] = (CACurrentMediaTime() - decodeStartedAt) * 1000
            fields["recovery_status"] = recovered.status
            fields["recovery_frames"] = recovered.frames
            fields["recovery_certified_blocks"] = recovered.certifiedBlocks
            fields["recovery_total_blocks"] = recovered.totalBlocks
            fields["code_detected"] = recovered.codeDetected
            if firstLiveDetectedSampleAt == nil,
               recovered.payload != nil || recovered.codeDetected || recovered.frames > 0 {
                firstLiveDetectedSampleAt = now
            }
            if let decoded = recovered.payload {
                let decodedAt = CACurrentMediaTime()
                let firstDetectedAt = firstLiveDetectedSampleAt ?? now
                let scanMilliseconds = (decodedAt - firstDetectedAt) * 1000
                fields["profile"] = decoded.profile
                fields["payload_length"] = decoded.data.count
                fields["corrected"] = decoded.corrected
                fields["erased"] = decoded.erased
                fields["scan_elapsed_ms"] = (decodedAt - (liveRecoveryStartedAt > 0 ? liveRecoveryStartedAt : now)) * 1000
                fields["recovery_episode_elapsed_ms"] = fields["scan_elapsed_ms"]
                fields["first_eligible_sample_received_at_monotonic_seconds"] = liveRecoveryStartedAt > 0 ? liveRecoveryStartedAt : now
                fields["first_detected_sample_received_at_monotonic_seconds"] = firstDetectedAt
                fields["first_detected_sample_to_decode_success_ms"] = scanMilliseconds
                fields["decode_success_at_monotonic_seconds"] = decodedAt
                fields["rust_final_call_ms"] = decoded.finalCallMilliseconds
                fields["first_frame_sensor_timestamp_seconds"] = liveRecoveryStartedAt > 0 ? firstLiveSensorTimestamp : sensorTimestamp
                acquisition.recordCameraEvent("decode_success", fields: fields)
                if !liveSavedSuccess {
                    liveSavedSuccess = true
                    archiveDecodeSample(pixels: base.assumingMemoryBound(to: UInt8.self), length: stride * height,
                        fields: fields, decoded: true, mode: .live)
                }
                liveFailureFrames.removeAll(keepingCapacity: false)
                liveRecoveryStartedAt = 0
                firstLiveDetectedSampleAt = nil
                output(.accepted(decoded, ticket: ticket, timing: LiveDecodeTiming(firstDetectedAt: firstDetectedAt,
                    decodedAt: decodedAt, captureSource: state.liveCaptureSource,
                    attemptID: captureID.uuidString.lowercased())))
                return
            }
            fields["error"] = recovered.message
            acquisition.recordCameraEvent(recovered.status == "pending" ? "live_frame_pending" : "decode_failure", fields: fields)
            if recovered.codeDetected || recovered.frames > 0 || liveRecoveryStartedAt > 0 {
                if liveRecoveryStartedAt == 0 {
                    liveRecoveryStartedAt = now
                    liveRecoveryAttempts = 0
                    liveRecoveryRejections = 0
                    firstLiveSensorTimestamp = frameFields["frame_timestamp_seconds"] as? Double ?? 0
                    state.lock.withLock { state.liveRecoveryActive = true }
                    acquisition.recordCameraEvent("automatic_recovery_started", fields: ["capture_source": state.liveCaptureSource,
                        "first_frame_sensor_timestamp_seconds": firstLiveSensorTimestamp,
                        "first_detected_sample_received_at_monotonic_seconds": firstLiveDetectedSampleAt ?? now])
                }
                liveRecoveryAttempts += 1
                if recovered.codeDetected, liveFailureFrames.count < 4, state.lock.withLock({
                    !failureArchiveBusy && (state.isReplayingFrames || CACurrentMediaTime() >= nextFailureArchiveAt)
                }) {
                    liveFailureFrames.append(LiveFailureFrame(pixels: Data(bytes: base, count: stride * height), metadata: fields))
                }
                liveRecoveryRejections = recovered.status == "error" ? liveRecoveryRejections + 1 : 0
                if recovered.frames >= recovered.maxFrames || liveRecoveryRejections >= 2 {
                    finishLiveRecovery(ticket: ticket, message: recovered.message ?? "Adjust the distance or angle slightly while keeping the whole code in view.")
                    return
                }
            }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.state.isCurrent(ticket) else { return }
                let diagnostic: String?
                if let certified = recovered.certifiedBlocks, let total = recovered.totalBlocks {
                    diagnostic = "Live recovery · \(certified) of \(total) blocks verified · \(recovered.frames) retained frames."
                } else if let message = recovered.message {
                    diagnostic = "\(width)×\(height) · \(message)"
                } else { diagnostic = nil }
                self.output(.presentation(CameraPresentation(status: self.state.scanningStatus, diagnostic: diagnostic,
                    processingImage: self.state.lock.withLock { self.state.liveRecoveryActive }), ticket: ticket))
            }
        } catch {
            guard state.isCurrent(ticket) else { return }
            acquisition.recordCameraEvent("decode_failure", fields: frameFields.merging(["error": error.localizedDescription,
                "decode_ms": (CACurrentMediaTime() - decodeStartedAt) * 1000]) { _, actual in actual })
            if liveRecoveryStartedAt > 0 { finishLiveRecovery(ticket: ticket, message: error.localizedDescription) }
        }
    }

    func finishLiveRecovery(ticket: Int, message: String) {
        guard state.isCurrent(ticket) else { return }
        archiveLiveFailure(message: message)
        clearRecovery()
        liveRecoveryStartedAt = 0
        state.lock.withLock { state.photoInFlight = false; state.liveRecoveryActive = false; state.cameraConfiguring = false }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.state.isCurrent(ticket) else { return }
            self.output(.presentation(CameraPresentation(status: "Looking for CW Code", diagnostic: message,
                processingImage: false), ticket: ticket))
        }
        acquisition.recordCameraEvent("automatic_recovery_finished", fields: ["capture_attempts": liveRecoveryAttempts, "message": message])
    }

    func receiveTransferFrame(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int,
                                      stride: Int, receivedAt: CFTimeInterval,
                                      decodeStartedAt: CFTimeInterval,
                                      frameFields: [String: Any], ticket: Int) {
        guard state.isCurrent(ticket) else { return }
        if transferFirstAttemptAt == nil { transferFirstAttemptAt = receivedAt }
        transferAttempts += 1
        var frameFields = frameFields
        frameFields["attempt_id"] = UUID().uuidString.lowercased()
        frameFields["capture_source"] = state.liveCaptureSource
        frameFields["sample_received_at_monotonic_seconds"] = receivedAt
        let decoded: DecodedPayload
        do {
            if transferScannerSession == nil {
                transferScannerSession = try TransferScannerSession()
            }
            guard let transferScannerSession else { throw CWFailure(message: "Cannot start transfer scanning.") }
            decoded = try transferScannerSession.decode(pixels: pixels, length: length, width: width, height: height, stride: stride)
        } catch {
            guard state.isCurrent(ticket) else { return }
            transferDecodeFailures += 1
            let detected = (error as? CWFailure)?.codeDetected ?? false
            let fields = frameFields.merging([
                "code_detected": detected,
                "decode_ms": (CACurrentMediaTime() - decodeStartedAt) * 1000,
                "sample_to_outcome_ms": (CACurrentMediaTime() - receivedAt) * 1000,
                "error": error.localizedDescription]) { _, value in value }
            let now = CACurrentMediaTime()
            if transferDecodeFailures == 1 || detected && !transferSavedFailure || now - lastTransferFailureLogAt >= 0.25 {
                lastTransferFailureLogAt = now
                acquisition.recordCameraEvent("decode_started", fields: frameFields)
                acquisition.recordCameraEvent("transfer_decode_failed", fields: fields.merging([
                    "decode_failures_total": transferDecodeFailures, "attempts_total": transferAttempts
                ]) { _, value in value })
            }
            if detected && !transferSavedFailure {
                transferSavedFailure = true
                archiveDecodeSample(pixels: pixels, length: length, fields: fields, decoded: false)
            }
            if now - lastTransferUIUpdateAt >= 0.25 {
                lastTransferUIUpdateAt = now
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.state.isCurrent(ticket) else { return }
                    self.output(.transferFailure(detected: detected, message: error.localizedDescription, ticket: ticket))
                }
            }
            return
        }
        guard state.isCurrent(ticket) else { return }
        do {
            let update = try transferReceiver.accept(decoded.data)
            let validatedAt = CACurrentMediaTime()
            guard state.isCurrent(ticket) else { return }
            let firstPacket = transferStartedAt == nil
            if (update.acceptedUnique || update.acceptedRepairSymbol) && firstPacket { transferStartedAt = receivedAt }
            guard let startedAt = transferStartedAt else { return }
            let elapsed = validatedAt - startedAt
            let megabits = Double(update.uniqueValidatedBytes) * 8 / max(elapsed, 0.000001) / 1_000_000
            var fields = frameFields
            fields.merge([
                "transfer_id": update.transferIDHex, "expected_sha256": update.expectedSHA256Hex,
                "original_bytes": update.originalLength, "chunk_bytes": update.chunkSize,
                "chunk_index": update.chunkIndex,
                "transfer_envelope": update.envelope,
                "repair_symbols": update.repairSymbols, "repaired_chunks": update.repairedChunks,
                "received_chunks": update.receivedChunks, "total_chunks": update.totalChunks,
                "missing_chunks": update.missingChunks, "duplicate_chunks": update.duplicateCount,
                "unique_validated_bytes": update.uniqueValidatedBytes, "attempts": transferAttempts,
                "decode_failures": transferDecodeFailures, "packet_failures": transferPacketFailures,
                "first_accepted_sample_received_at_monotonic_seconds": startedAt,
                "sample_received_at_monotonic_seconds": receivedAt,
                "validated_at_monotonic_seconds": validatedAt,
                "first_attempt_at_monotonic_seconds": transferFirstAttemptAt ?? receivedAt,
                "elapsed_ms": elapsed * 1000, "decode_ms": (validatedAt - decodeStartedAt) * 1000,
                "rust_final_call_ms": decoded.finalCallMilliseconds,
                "sample_to_outcome_ms": (validatedAt - receivedAt) * 1000,
                "application_mbps": megabits, "capture_source": state.liveCaptureSource,
                "timing_scope": "first accepted source or repair buffer receipt through validation; includes first decode, later failures, duplicates, repair and reassembly"
            ]) { _, value in value }
            let complete = update.completedData != nil
            if let sourceIndex = update.sourceChunkIndex { fields["source_chunk_index"] = sourceIndex }
            if let missing = update.missingIndices { fields["missing_chunk_indices"] = missing }
            let updateUI = complete || firstPacket || validatedAt - lastTransferUIUpdateAt >= 0.25
            if updateUI { lastTransferUIUpdateAt = validatedAt }
            acquisition.recordCameraEvent("decode_started", fields: frameFields)
            acquisition.recordCameraEvent(complete ? "transfer_complete" : update.duplicate ? "transfer_duplicate" : update.acceptedRepairSymbol ? "transfer_repair_symbol" : "transfer_chunk", fields: fields)
            if (update.acceptedUnique || update.acceptedRepairSymbol) && !transferSavedSuccess {
                transferSavedSuccess = true
                archiveDecodeSample(pixels: pixels, length: length, fields: fields, decoded: true)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.state.isCurrent(ticket) else { return }
                self.output(.transfer(update, startedAt: startedAt, validatedAt: validatedAt, updateUI: updateUI, ticket: ticket))
            }
            if let data = update.completedData {
                var payload = DecodedPayload(data: data, profile: decoded.profile,
                    finalCallMilliseconds: decoded.finalCallMilliseconds, scanMilliseconds: elapsed * 1000,
                    corrected: decoded.corrected, erased: decoded.erased)
                payload.transfer = TransferStatistics(bytes: data.count, elapsedSeconds: elapsed,
                    chunks: update.totalChunks, duplicates: update.duplicateCount, decodeFailures: transferDecodeFailures,
                    repairSymbols: update.repairSymbols, repairedChunks: update.repairedChunks)
                output(.accepted(payload, ticket: ticket, timing: nil))
            }
        } catch {
            guard state.isCurrent(ticket) else { return }
            transferPacketFailures += 1
            acquisition.recordCameraEvent("decode_started", fields: frameFields)
            acquisition.recordCameraEvent("transfer_packet_rejected", fields: frameFields.merging(["error": error.localizedDescription,
                "sample_received_at_monotonic_seconds": receivedAt, "packet_failures": transferPacketFailures]) { _, value in value })
            DispatchQueue.main.async { [weak self] in
                guard let self, self.state.isCurrent(ticket) else { return }
                self.output(.presentation(CameraPresentation(diagnostic: error.localizedDescription), ticket: ticket))
            }
        }
    }
}

extension CameraDecodeCoordinator {
    func archiveLiveFailure(message: String) {
        let availability = state.lock.withLock {
            (busy: failureArchiveBusy, wait: state.isReplayingFrames ? 0 : max(0, nextFailureArchiveAt - CACurrentMediaTime()))
        }
        guard !availability.busy, availability.wait == 0 else {
            liveFailureFrames.removeAll(keepingCapacity: false)
            var fields: [String: Any] = ["reason": availability.busy ? "writer_busy" : "archive_cooldown",
                "capture_source": state.liveCaptureSource]
            if availability.busy { fields["archive_cooldown_after_successful_write_seconds"] = 30 }
            else { fields["archive_available_in_seconds"] = availability.wait }
            acquisition.recordCameraEvent("live_failure_archive_skipped", fields: fields)
            return
        }
        guard !liveFailureFrames.isEmpty else { return }
        let frames = liveFailureFrames
        liveFailureFrames.removeAll(keepingCapacity: false)
        guard state.lock.withLock({ () -> Bool in
            guard !failureArchiveBusy else { return false }
            failureArchiveBusy = true
            return true
        }) else { return }
        let metadata: [String: Any] = ["schema_version": 1,
            "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
            "capture_source": state.liveCaptureSource, "pixel_format": "BGRA8", "exact_pixels": true,
            "started_at_monotonic_seconds": liveRecoveryStartedAt,
            "elapsed_ms": (CACurrentMediaTime() - liveRecoveryStartedAt) * 1000,
            "first_frame_sensor_timestamp_seconds": frames.first?.metadata["frame_timestamp_seconds"] ?? firstLiveSensorTimestamp,
            "capture_attempts": liveRecoveryAttempts, "failure": message,
            "archived_at": Date().timeIntervalSince1970]
        var attemptIDs = [String]()
        var seenAttemptIDs = Set<String>()
        for frame in frames {
            guard let attemptID = frame.metadata["attempt_id"] as? String,
                  seenAttemptIDs.insert(attemptID).inserted else { continue }
            attemptIDs.append(attemptID)
        }
        let sessionQueue = acquisition.sessionQueue
        let diagnostics = acquisition.diagnostics
        failureArchiveQueue.async { [weak self] in
            defer {
                if let self { self.state.lock.withLock { self.failureArchiveBusy = false } }
            }
            do {
                sessionQueue.sync {}
                var archiveMetadata = metadata
                archiveMetadata["attempt_diagnostics"] = diagnostics.matchingAttemptDiagnostics(for: attemptIDs)
                try LiveFailureArchive.save(frames, metadata: archiveMetadata)
                if let self { self.state.lock.withLock { self.nextFailureArchiveAt = CACurrentMediaTime() + 30 } }
                self?.acquisition.recordCameraEvent("live_failure_archived", fields: ["frame_count": frames.count,
                    "archive": "LiveFailures/latest", "exact_pixels": true, "archive_cooldown_seconds": 30])
            } catch {
                self?.acquisition.recordCameraEvent("live_failure_archive_error", fields: ["error": error.localizedDescription])
            }
        }
    }

    func archiveDecodeSample(pixels: UnsafePointer<UInt8>, length: Int, fields: [String: Any], decoded: Bool, mode: ScanMode = .transfer) {
        guard !state.isReplayingFrames else { return }
        let frame = LiveFailureFrame(pixels: Data(bytes: pixels, count: length), metadata: fields)
        let path = "TransferSamples/" + (mode == .live ? "live-success" : decoded ? "success" : "failure")
        let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent(path, isDirectory: true)
        let diagnostics = acquisition.diagnostics
        let sessionQueue = acquisition.sessionQueue
        failureArchiveQueue.async { [weak self] in
            do {
                sessionQueue.sync {}
                let attemptID = fields["attempt_id"] as? String ?? ""
                let metadata: [String: Any] = ["schema_version": 1, "capture_source": "video_bgra",
                    "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
                    "pixel_format": "BGRA8", "exact_pixels": true, "decoded_chunk": decoded && mode == .transfer,
                    "decoded_payload": decoded, "scan_mode": mode.rawValue,
                    "archived_at": Date().timeIntervalSince1970,
                    "attempt_diagnostics": diagnostics.matchingAttemptDiagnostics(for: [attemptID])]
                try LiveFailureArchive.save([frame], metadata: metadata, directory: directory)
                self?.acquisition.recordCameraEvent(mode == .live ? "live_sample_archived" : "transfer_sample_archived",
                    fields: ["decoded_chunk": decoded && mode == .transfer, "archive": path + "/latest"])
            } catch {
                self?.acquisition.recordCameraEvent("decode_sample_archive_error", fields: ["scan_mode": mode.rawValue, "error": error.localizedDescription])
            }
        }
    }
}
