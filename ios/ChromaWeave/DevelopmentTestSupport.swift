#if CW_DEVELOPMENT
import Foundation
import UIKit
import ImageIO
import UniformTypeIdentifiers

extension ScanDiagnosticStore {
    static func selfTest() throws -> Data {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("scan-diagnostics-test-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("scan-diagnostics.json")
        let inventory: [String: Any] = [
            "inventory_device_count": 1,
            "candidate_format_count": 1,
            "devices": [[
                "device_unique_id": "self-test-device",
                "formats": [["horizontal_video_field_of_view_degrees": 63.0]]
            ]]
        ]
        let original = ScanDiagnosticStore(url: url, maximumBytes: 2_048)
        original.recordVideoFormatInventory(inventory)
        for index in 0..<20 { original.record(["index": index, "cause": String(repeating: "x", count: 200)]) }
        let first = try original.queue.sync {
            original.pendingWrite?.cancel()
            original.pendingWrite = nil
            return try original.writeSnapshot()
        }
        let restored = ScanDiagnosticStore(url: url, maximumBytes: 2_048)
        let second = try restored.queue.sync { try restored.writeSnapshot() }
        let firstObject = try JSONSerialization.jsonObject(with: first) as? [String: Any]
        let secondObject = try JSONSerialization.jsonObject(with: second) as? [String: Any]
        func candidateFormatCount(in document: [String: Any]?) -> Int? {
            guard let inventoryDocument = document?["video_format_inventory"] as? [String: Any],
                  let inventory = inventoryDocument["inventory"] as? [String: Any] else { return nil }
            return inventory["candidate_format_count"] as? Int
        }
        guard first == second,
              candidateFormatCount(in: firstObject) == 1,
              candidateFormatCount(in: secondObject) == 1,
              let events = secondObject?["events"] as? [[String: Any]],
              !events.isEmpty, events.count < 20, events.last?["index"] as? Int == 19,
              restored.eventBytes <= 2_048 else { throw CWFailure(message: "Diagnostic store round-trip or eviction failed.") }
        let frozen = try restored.queue.sync {
            try restored.snapshot(capture: ["capture_id": "self-test", "frame_timestamp_seconds": 1.5, "decoded": false])
        }
        restored.record(["index": 20])
        let live = try restored.queue.sync {
            restored.pendingWrite?.cancel()
            restored.pendingWrite = nil
            return try restored.writeSnapshot()
        }
        let frozenObject = try JSONSerialization.jsonObject(with: frozen) as? [String: Any]
        let liveObject = try JSONSerialization.jsonObject(with: live) as? [String: Any]
        guard (frozenObject?["captured_frame"] as? [String: Any])?["capture_id"] as? String == "self-test",
              candidateFormatCount(in: frozenObject) == 1,
              (frozenObject?["events"] as? [[String: Any]])?.last?["index"] as? Int == 19,
              candidateFormatCount(in: liveObject) == 1,
              (liveObject?["events"] as? [[String: Any]])?.last?["index"] as? Int == 20 else {
            throw CWFailure(message: "Inventory or captured-frame diagnostics changed after reload or live events.")
        }
        let pairedDirectory = directory.appendingPathComponent("paired-attempts", isDirectory: true)
        try FileManager.default.createDirectory(at: pairedDirectory, withIntermediateDirectories: true)
        let pairedStore = ScanDiagnosticStore(url: pairedDirectory.appendingPathComponent("scan-diagnostics.json"), maximumBytes: 2_048)
        pairedStore.record(["event": "decode_started", "attempt_id": "attempt-matched"])
        pairedStore.record(["event": "decode_failure", "attempt_id": "attempt-matched"])
        pairedStore.record(["event": "decode_started", "attempt_id": "attempt-unrelated"])
        pairedStore.record(["event": "focus_adjustment", "attempt_id": "attempt-unrelated"])
        pairedStore.record(["event": "decode_failure", "attempt_id": "attempt-unrelated"])
        let paired = pairedStore.matchingAttemptDiagnostics(for: ["attempt-matched", "attempt-missing"])
        let pairedBytes = try JSONSerialization.data(withJSONObject: paired, options: [.sortedKeys])
        let pairedEvents = paired["events"] as? [[String: Any]] ?? []
        let pairedMissing = paired["missing_attempt_ids"] as? [String] ?? []
        let incomplete = paired["incomplete_attempt_events"] as? [[String: Any]] ?? []
        pairedStore.record(["event": "decode_failure", "attempt_id": "attempt-matched"])
        let pairedAfterEvent = try JSONSerialization.data(withJSONObject: paired, options: [.sortedKeys])
        let refreshed = pairedStore.matchingAttemptDiagnostics(for: ["attempt-matched", "attempt-missing"])
        let refreshedEvents = refreshed["events"] as? [[String: Any]] ?? []
        guard pairedBytes == pairedAfterEvent,
              paired["camera_sampling"] as? String == Self.cameraSamplingScope,
              pairedEvents.count == 2,
              pairedEvents.allSatisfy({ $0["attempt_id"] as? String == "attempt-matched" }),
              pairedMissing == ["attempt-missing"], incomplete.count == 1,
              incomplete.first?["attempt_id"] as? String == "attempt-missing",
              refreshedEvents.count == 3 else {
            throw CWFailure(message: "Paired attempt diagnostics included unrelated events or changed after capture.")
        }
        pairedStore.queue.sync { pairedStore.pendingWrite?.cancel(); pairedStore.pendingWrite = nil }
        return try JSONSerialization.data(withJSONObject: ["exact_disk_roundtrip": true, "eviction": true,
                                                           "frozen_capture_snapshot": true,
                                                           "video_format_inventory_survives_export_reload": true,
                                                           "attempt_diagnostic_events_match_ids": true,
                                                           "attempt_diagnostic_snapshot_immutable": true,
                                                           "retained_events": events.count, "bytes": second.count], options: [.prettyPrinted])
    }
}

extension CapturedPhotoArchive {
    static func selfTest() throws -> [String: Any] {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("photo-archive-test-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let encoded = try Codec.encode(Codec.sample, cellPixels: 2)
        let (prepared, _) = try Codec.scanPhoto(encoded.png, captureID: UUID())
        var frame = prepared
        let diagnostics = try JSONSerialization.data(withJSONObject: ["capture_id": frame.id.uuidString.lowercased(), "synthetic": true])
        frame.diagnostics = .success(diagnostics)
        try save(frame, directory: directory)
        let saved = directory.appendingPathComponent(frame.id.uuidString.lowercased())
        let png = try Data(contentsOf: saved.appendingPathComponent("\(frame.exportStem).png"))
        let json = try Data(contentsOf: saved.appendingPathComponent("\(frame.exportStem)-diagnostics.json"))
        guard png == frame.image.pngData(), json == diagnostics,
              try FileManager.default.contentsOfDirectory(atPath: saved.path).count == 2 else {
            throw CWFailure(message: "Saved photo/diagnostics pair differs from the finalized capture.")
        }
        let blocked = directory.appendingPathComponent("not-a-directory")
        try Data([1]).write(to: blocked, options: .atomic)
        let failedWrite = Result { try save(frame, directory: blocked) }
        guard case .failure = failedWrite, frame.image.pngData() == png,
              try frame.diagnostics?.get() == diagnostics else { throw CWFailure(message: "Archive failure did not preserve manual exports.") }
        return ["archive_png_exact": true, "archive_diagnostics_exact": true,
            "archive_complete_pair": true, "archive_failure_retains_exports": true]
    }
}

extension Codec {
    static func photoSelfTest() throws -> Data {
        guard photoMaximumSide(width: 4032, height: 3024) == 4032,
              photoMaximumSide(width: 8064, height: 6048) < 8064,
              photoMaximumSide(width: 6048, height: 8064) == photoMaximumSide(width: 8064, height: 6048),
              maximumPayloadBytes(profile: 3) == 198_420,
              maximumPayloadBytes(profile: 4) == 258_324,
              [0, 1, 2].allSatisfy({
                  maximumPayloadBytes(profile: $0) == nil && maximumInputBytes(profile: $0, losslessCompression: true) == nil
              }) else {
            throw CWFailure(message: "Photo resolution or CW Luma10 profile bounds failed.")
        }
        let payload = sample
        let encoded = try encode(payload, cellPixels: 2)
        let firstCaptureID = UUID()
        let (frame, fields) = try scanPhoto(encoded.png, captureID: firstCaptureID)
        guard frame.id == firstCaptureID, frame.decodedPayload?.data == payload,
              frame.recoveryFrames == 1, frame.recoveryCaptureIDs == [firstCaptureID.uuidString.lowercased()],
              frame.recoverySessionID != nil,
              frame.width == encoded.side, frame.height == encoded.side,
              fields["resize_applied"] as? Bool == false,
              let png = frame.image.pngData() else { throw CWFailure(message: "Still-photo preparation changed the native payload.") }
        let repeatedCaptureID = UUID()
        let (repeated, _) = try scanPhoto(encoded.png, captureID: repeatedCaptureID)
        guard repeated.decodedPayload?.data == payload,
              repeated.recoveryFrames == 1,
              repeated.recoveryCaptureIDs == [repeatedCaptureID.uuidString.lowercased()],
              repeated.recoverySessionID != frame.recoverySessionID else {
            throw CWFailure(message: "Independent photo scans reused recovery state or capture IDs.")
        }
        let exportedCaptureID = UUID()
        let (exported, _) = try scanPhoto(png, captureID: exportedCaptureID)
        guard exported.decodedPayload?.data == payload,
              exported.recoveryFrames == 1,
              exported.recoveryCaptureIDs == [exportedCaptureID.uuidString.lowercased()],
              exported.recoverySessionID != frame.recoverySessionID else {
            throw CWFailure(message: "Photo PNG export changed the payload or reused its recovery session.")
        }
        let compressible = Data(repeating: 0x41, count: 2048)
        let compressed = try encode(compressible, cellPixels: 2, losslessCompression: true)
        let compressedCaptureID = UUID()
        let (compressedFrame, _) = try scanPhoto(compressed.png, captureID: compressedCaptureID)
        guard compressed.compression == "zstd", compressed.storedBytes < compressible.count,
              compressedFrame.decodedPayload?.data == compressible,
              compressedFrame.recoveryFrames == 1,
              compressedFrame.recoveryCaptureIDs == [compressedCaptureID.uuidString.lowercased()] else {
            throw CWFailure(message: "Compressed encoding changed the original payload.")
        }
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let blank = UIGraphicsImageRenderer(size: CGSize(width: 32, height: 16), format: format).image { context in
            UIColor.white.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 32, height: 16))
        }
        let oriented = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(oriented, UTType.jpeg.identifier as CFString, 1, nil),
              let image = blank.cgImage else { throw CWFailure(message: "Cannot create photo orientation fixture.") }
        CGImageDestinationAddImage(destination, image, [kCGImagePropertyOrientation: 6] as CFDictionary)
        guard CGImageDestinationFinalize(destination) else { throw CWFailure(message: "Cannot encode photo orientation fixture.") }
        let blankCaptureID = UUID()
        let (unreadable, _) = try scanPhoto(oriented as Data, captureID: blankCaptureID)
        guard unreadable.width == 16, unreadable.height == 32, unreadable.decodedPayload == nil,
              unreadable.recoveryFrames == 0, unreadable.recoveryCaptureIDs?.isEmpty == true,
              unreadable.decodeError != nil, unreadable.image.pngData() != nil else {
            throw CWFailure(message: "Photo orientation or failed-decode image retention failed.")
        }
        var fallbackRecoveryCalls = 0
        let callbackCaptureID = UUID()
        let (callbackFrame, _) = try scanPhoto(oriented as Data, captureID: callbackCaptureID) { _, _, _, _, _ in
            fallbackRecoveryCalls += 1
            return PhotoRecoveryResult(status: "pending", payload: nil, message: nil,
                frames: 1, maxFrames: 4, certifiedBlocks: 0, totalBlocks: 1,
                sessionID: UUID().uuidString.lowercased(),
                captureIDs: [callbackCaptureID.uuidString.lowercased()], codeDetected: true)
        }
        guard fallbackRecoveryCalls == 1, callbackFrame.recoveryStatus == "pending" else {
            throw CWFailure(message: "The supplied photo recovery callback was not used exactly once.")
        }
        return try JSONSerialization.data(withJSONObject: ["native_payload_exact": true, "export_payload_exact": true,
            "capture_id_preserved": true, "no_upscale": true, "large_photo_bound": true,
            "orientation_normalized": true, "failed_decode_image_retained": true,
            "compressed_payload_exact": true, "one_shot_recovery_session": true,
            "independent_recovery_sessions": true, "capture_id_isolation": true,
            "empty_photo_recovery_admission": true, "supplied_recovery_callback_once": true,
            "profile_3_4_capacities_stable": true, "removed_profiles_rejected": true,
            "source": "synthetic_image_preparation_only", "physical_camera_validated": false], options: [.prettyPrinted])
    }

}
#endif
