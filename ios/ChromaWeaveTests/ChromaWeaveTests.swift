import XCTest
import UIKit
import AVFoundation
@testable import ChromaWeave

final class ChromaWeaveTests: XCTestCase {
    func testTransferProtocolAndRepairInvariants() throws {
        let report = try TransferReceiver.selfTest()
        XCTAssertEqual(report["passed"] as? Bool, true)
        let repairReport = try XCTUnwrap(report["cwt"] as? [String: Any])
        XCTAssertEqual(repairReport["invalid_magic_rejected"] as? Bool, true)
    }

    func testDiagnosticEvictionPersistenceAndFrozenAttempts() throws {
        let report = try JSONSerialization.jsonObject(with: ScanDiagnosticStore.selfTest()) as? [String: Any]
        XCTAssertEqual(report?["exact_disk_roundtrip"] as? Bool, true)
        XCTAssertEqual(report?["attempt_diagnostic_snapshot_immutable"] as? Bool, true)
    }

    func testPhotoNormalizationAndIndependentRecovery() throws {
        let report = try JSONSerialization.jsonObject(with: Codec.photoSelfTest()) as? [String: Any]
        XCTAssertEqual(report?["independent_recovery_sessions"] as? Bool, true)
        XCTAssertEqual(report?["empty_photo_recovery_admission"] as? Bool, true)
    }

    func testArchivePairsAndFailureRetainExports() throws {
        let report = try CapturedPhotoArchive.selfTest()
        XCTAssertEqual(report["archive_complete_pair"] as? Bool, true)
        XCTAssertEqual(report["archive_failure_retains_exports"] as? Bool, true)
    }

    func testTypedResponseRejectsIncompleteSuccess() throws {
        let response = try Codec.parseResponse(Data(#"{"status":"decoded","payload":[1],"modulation":"Luma10"}"#.utf8), as: CodecResponse.self)
        XCTAssertThrowsError(try response.decodedPayload())
        XCTAssertThrowsError(try Codec.parseResponse(Data(#"{"status":"decoded","payload":[256]}"#.utf8), as: CodecResponse.self))
        XCTAssertThrowsError(try Codec.parseResponse(Data(#"{"status":"unknown"}"#.utf8), as: CodecResponse.self))
    }

    func testRecoveryResponseRetainsOnlyAdmittedCaptures() throws {
        let response = try Codec.parseResponse(Data(#"{"status":"error","frames":0,"max_frames":4,"code_detected":false}"#.utf8), as: CodecResponse.self)
        let recovery = try response.recovery(sessionID: UUID(), captureIDs: [UUID()])
        XCTAssertTrue(recovery.captureIDs.isEmpty)
        XCTAssertFalse(recovery.codeDetected)
        let invalid = try Codec.parseResponse(Data(#"{"status":"pending","frames":2,"max_frames":4}"#.utf8), as: CodecResponse.self)
        XCTAssertThrowsError(try invalid.recovery(sessionID: UUID(), captureIDs: [UUID()]))
    }

    func testExplicitDiagnosticResetPreservesPayloadsAndFixtures() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let capture = directory.appendingPathComponent("Captures")
        try FileManager.default.createDirectory(at: capture, withIntermediateDirectories: true)
        try Data([1]).write(to: capture.appendingPathComponent("saved.png"))
        let payload = directory.appendingPathComponent("recovered.gif")
        try Data([2]).write(to: payload)
        let fixture = directory.appendingPathComponent("transfer-regression")
        try FileManager.default.createDirectory(at: fixture, withIntermediateDirectories: true)
        try Data([3]).write(to: fixture.appendingPathComponent("manifest.json"))
        let partial = AppOwnedDiagnosticCleanup.reset(documents: directory, removeItem: { url in
            if url.path == capture.path { throw CWFailure(message: "Blocked capture") }
            try FileManager.default.removeItem(at: url)
        })
        XCTAssertFalse(partial.succeeded)
        XCTAssertTrue(FileManager.default.fileExists(atPath: capture.path))
        XCTAssertTrue(AppOwnedDiagnosticCleanup.reset(documents: directory).succeeded)
        XCTAssertFalse(FileManager.default.fileExists(atPath: capture.path))
        XCTAssertEqual(try Data(contentsOf: payload), Data([2]))
        XCTAssertEqual(try Data(contentsOf: fixture.appendingPathComponent("manifest.json")), Data([3]))
    }

    func testCameraLifecycleRestartPreservesDisplayedResultAndCapture() {
        let payload = DecodedPayload(data: Data([1, 2, 3]), profile: "Luma10", finalCallMilliseconds: 1,
            corrected: 0, erased: 0)
        let capture = CapturedFrame(image: UIImage(), width: 1, height: 1)
        let cases: [(DecodedPayload?, CapturedFrame?)] = [(payload, nil), (nil, capture)]
        for (displayedPayload, displayedCapture) in cases {
            let scanner = CameraScanner(configuration: .savedFrames)
            scanner.result = displayedPayload
            scanner.capturedFrame = displayedCapture
            scanner.stop()
            scanner.decoding.decodeQueue.sync {}
            let stoppedGeneration = scanner.state.lock.withLock { scanner.state.generation }
            scanner.start()
            XCTAssertEqual(scanner.result?.id, displayedPayload?.id)
            XCTAssertEqual(scanner.capturedFrame?.id, displayedCapture?.id)
            XCTAssertFalse(scanner.state.lock.withLock { scanner.state.cameraWanted })
            XCTAssertEqual(scanner.state.lock.withLock { scanner.state.generation }, stoppedGeneration)
        }
    }

    func testStopInvalidatesQueuedDecodeReset() {
        let scanner = CameraScanner(configuration: .savedFrames)
        scanner.state.lock.withLock { scanner.state.cameraWanted = true; scanner.state.generation = 1 }
        scanner.decoding.decodeQueue.sync { scanner.decoding.reset(for: 1) }
        let oldTicket = scanner.state.lock.withLock { scanner.state.generation }
        scanner.stop()
        scanner.decoding.decodeQueue.sync {
            XCTAssertFalse(scanner.isCurrent(oldTicket))
            XCTAssertNil(scanner.decoding.recoverySession)
            XCTAssertNil(scanner.decoding.transferStartedAt)
            XCTAssertEqual(scanner.decoding.decodeStateGeneration, scanner.state.lock.withLock { scanner.state.generation })
        }
    }

    func testRejectedRecoveryDoesNotClaimCapture() throws {
        let session = try RecoverySession()
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let blank = UIGraphicsImageRenderer(size: CGSize(width: 32, height: 16), format: format).image { context in
            UIColor.white.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 32, height: 16))
        }
        let rejectedID = UUID()
        let (rejected, _) = try Codec.scanPhoto(XCTUnwrap(blank.pngData()), captureID: rejectedID) { pixels, length, width, height, stride in
            try session.decode(pixels: pixels, length: length, width: width, height: height, stride: stride, captureID: rejectedID)
        }
        XCTAssertEqual(rejected.recoveryFrames, 0)
        XCTAssertTrue(session.captureIDs.isEmpty)
        let acceptedID = UUID()
        let encoded = try Codec.encode(Codec.sample, cellPixels: 2)
        let (accepted, _) = try Codec.scanPhoto(encoded.png, captureID: acceptedID) { pixels, length, width, height, stride in
            try session.decode(pixels: pixels, length: length, width: width, height: height, stride: stride, captureID: acceptedID)
        }
        XCTAssertEqual(accepted.decodedPayload?.data, Codec.sample)
        XCTAssertEqual(session.captureIDs, [acceptedID])
    }

    func testGenerationResetRetainsTransferScannerAndClearsItsGeometryRevision() throws {
        let owner = CameraScanner(configuration: .savedFrames)
        let coordinator = owner.decoding
        let sample = try Codec.savedImageSampleBuffer(Codec.encode(Codec.sample, cellPixels: 2).png,
            timestamp: CMTime(value: 1, timescale: 60))
        owner.state.lock.withLock {
            owner.state.generation = 1
            owner.state.cameraWanted = true
            owner.state.transferDecoding = true
            owner.state.lensRevision = 9
        }
        try coordinator.decodeQueue.sync {
            coordinator.processVideoFrame(try XCTUnwrap(coordinator.videoFrame(sample)))
            let scanner = try XCTUnwrap(coordinator.transferScannerSession)
            XCTAssertEqual(coordinator.transferScannerLensRevision, 9)
            coordinator.reset(for: 2)
            XCTAssertTrue(coordinator.transferScannerSession === scanner)
            XCTAssertNil(coordinator.transferScannerLensRevision)
            XCTAssertEqual(coordinator.transferAttempts, 0)
        }
    }

    func testResetReleasesOwnedRecoveryAndIsIdempotentWithinGeneration() throws {
        let owner = CameraScanner(configuration: .savedFrames)
        let coordinator = owner.decoding
        let encoded = try Codec.encode(Codec.sample, cellPixels: 2)
        try coordinator.decodeQueue.sync {
            coordinator.reset(for: 1)
            _ = try Codec.scanPhoto(encoded.png, captureID: UUID()) { pixels, length, width, height, stride in
                try coordinator.recover(pixels: pixels, length: length, width: width, height: height,
                    stride: stride, captureID: UUID())
            }
            weak var observed: RecoverySession?
            observed = coordinator.recoverySession
            XCTAssertNotNil(observed)
            coordinator.reset(for: 1)
            XCTAssertTrue(coordinator.recoverySession === observed)
            coordinator.reset(for: 2)
            XCTAssertNil(observed)
        }
    }

    func testTransferBufferBoundAndManualCaptureProtection() throws {
        let owner = CameraScanner(configuration: .savedFrames)
        let coordinator = owner.decoding
        let encoded = try Codec.encode(Codec.sample, cellPixels: 2)
        let samples = try (1...3).map {
            try Codec.savedImageSampleBuffer(encoded.png, timestamp: CMTime(value: Int64($0), timescale: 1))
        }
        owner.state.lock.withLock {
            owner.state.cameraWanted = true
            owner.state.transferDecoding = true
        }
        coordinator.decodeQueue.sync {
            for index in [0, 1, 2, 0] { coordinator.enqueueVideoFrame(samples[index]) }
            owner.state.lock.withLock {
                XCTAssertEqual(coordinator.pendingVideoFrames.map(\.sensorTimestamp), [3, 1])
                XCTAssertEqual(coordinator.videoBuffersOverwritten, 2)
                XCTAssertTrue(coordinator.videoWorkerScheduled)
                owner.state.captureRequested = true
            }
            coordinator.enqueueVideoFrame(samples[1])
            coordinator.enqueueVideoFrame(samples[2])
            let stale = owner.state.lock.withLock { () -> CameraVideoFrame in
                XCTAssertEqual(coordinator.pendingVideoFrames.count, 1)
                XCTAssertTrue(coordinator.pendingVideoFrames[0].capture)
                XCTAssertEqual(coordinator.pendingVideoFrames[0].sensorTimestamp, 2)
                return coordinator.pendingVideoFrames[0]
            }
            owner.stop()
            coordinator.processVideoFrame(stale)
            owner.state.lock.withLock { XCTAssertTrue(coordinator.pendingVideoFrames.isEmpty) }
            XCTAssertNil(coordinator.transferReceiver.transferIDHex)
        }
    }

    func testLiveBufferRetainsOnlyNewestFrame() throws {
        let owner = CameraScanner(configuration: .savedFrames)
        let coordinator = owner.decoding
        let encoded = try Codec.encode(Codec.sample, cellPixels: 2)
        let samples = try (1...3).map {
            try Codec.savedImageSampleBuffer(encoded.png, timestamp: CMTime(value: Int64($0), timescale: 1))
        }
        owner.state.lock.withLock { owner.state.cameraWanted = true }
        coordinator.decodeQueue.sync {
            for sample in samples { coordinator.enqueueVideoFrame(sample) }
            owner.state.lock.withLock {
                XCTAssertEqual(coordinator.pendingVideoFrames.map(\.sensorTimestamp), [3])
                XCTAssertEqual(coordinator.videoBuffersOverwritten, 2)
            }
            owner.stop()
        }
    }

    func testStopPreventsAlreadyDecodedPayloadPublication() {
        let owner = CameraScanner(configuration: .savedFrames)
        let payload = DecodedPayload(data: Data([1, 2, 3]), profile: "Luma10", finalCallMilliseconds: 1,
            corrected: 0, erased: 0)
        owner.state.lock.withLock { owner.state.cameraWanted = true }
        let ticket = owner.state.lock.withLock { owner.state.generation }
        owner.decoding.decodeQueue.sync { owner.accept(payload, ticket: ticket) }
        owner.stop()
        let published = expectation(description: "Queued result delivery completed")
        DispatchQueue.main.async {
            XCTAssertNil(owner.result)
            XCTAssertFalse(owner.state.lock.withLock { owner.state.cameraWanted })
            published.fulfill()
        }
        wait(for: [published], timeout: 2)
    }
}
