import Foundation

final class ScanDiagnosticStore {
    static let cameraSamplingScope = "Camera properties are sampled asynchronously at camera.sampled_at, not at sensor exposure time."
    static let lensPositionScope = "Lens position 0 is nearest and 1 is furthest; 1 is also the default. Values do not encode physical distance or prove focus, and capture-device and constituent readings are recorded separately."
    let queue = DispatchQueue(label: "cw.camera.diagnostics", qos: .utility)
    let url: URL
    let videoFormatURL: URL
    let maximumBytes: Int
    var events = [Data]()
    var eventBytes = 0
    var pendingWrite: DispatchWorkItem?
    var videoFormatInventory: [String: Any]?
    var writeError: Error?

    init(url: URL = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("scan-diagnostics.json"), maximumBytes: Int = 1_800_000) {
        self.url = url
        videoFormatURL = url.deletingLastPathComponent().appendingPathComponent("camera-video-formats.json")
        self.maximumBytes = maximumBytes
        queue.async {
            if let inventoryData = try? Data(contentsOf: self.videoFormatURL),
               let inventoryDocument = try? JSONSerialization.jsonObject(with: inventoryData) as? [String: Any] {
                self.videoFormatInventory = inventoryDocument
            }
            guard let data = try? Data(contentsOf: url),
                  let document = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let previous = document["events"] as? [[String: Any]] else { return }
            for event in previous {
                if let data = try? JSONSerialization.data(withJSONObject: event, options: [.sortedKeys]) { self.append(data) }
            }
        }
    }

    func record(_ event: [String: Any]) {
        queue.async { [self] in
            do {
                var event = event
                event["app_build"] = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown"
                self.append(try JSONSerialization.data(withJSONObject: event, options: [.sortedKeys]))
                if event["event"] as? String == "transfer_complete", event["capture_source"] as? String == "video_bgra" {
                    let historyURL = url.deletingLastPathComponent().appendingPathComponent("completed-transfers.json")
                    var history = [[String: Any]]()
                    if FileManager.default.fileExists(atPath: historyURL.path) {
                        history = try JSONSerialization.jsonObject(with: Data(contentsOf: historyURL)) as? [[String: Any]] ?? []
                    }
                    history.append(event)
                    try JSONSerialization.data(withJSONObject: Array(history.suffix(20)), options: [.prettyPrinted, .sortedKeys])
                        .write(to: historyURL, options: .atomic)
                }
                guard self.pendingWrite == nil else { return }
                let write = DispatchWorkItem { [weak self] in self?.persist() }
                self.pendingWrite = write
                self.queue.asyncAfter(deadline: .now() + 1, execute: write)
            } catch { self.writeError = error }
        }
    }

    func recordVideoFormatInventory(_ inventory: [String: Any]) {
        queue.async { [self] in
            do {
                let document: [String: Any] = [
                    "schema_version": 1,
                    "captured_at": Date().timeIntervalSince1970,
                    "app_version": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown",
                    "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
                    "inventory": inventory
                ]
                videoFormatInventory = document
                let data = try JSONSerialization.data(withJSONObject: document, options: [.sortedKeys])
                try data.write(to: videoFormatURL, options: .atomic)
            } catch { writeError = error }
        }
    }

    func matchingAttemptDiagnostics(for attemptIDs: [String]) -> [String: Any] {
        queue.sync {
            var orderedIDs = [String]()
            var requestedIDs = Set<String>()
            for attemptID in attemptIDs where requestedIDs.insert(attemptID).inserted {
                orderedIDs.append(attemptID)
            }
            let requested = Set(orderedIDs)
            let matchingEventTypes: Set<String> = ["decode_started", "decode_success", "decode_failure", "live_frame_pending",
                "transfer_chunk", "transfer_duplicate", "transfer_complete", "transfer_decode_failed", "transfer_packet_rejected"]
            var matchedEvents = [[String: Any]]()
            var eventsByAttemptID = [String: [[String: Any]]]()
            for data in events {
                guard let event = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let attemptID = event["attempt_id"] as? String,
                      requested.contains(attemptID),
                      let eventType = event["event"] as? String,
                      matchingEventTypes.contains(eventType) else { continue }
                matchedEvents.append(event)
                eventsByAttemptID[attemptID, default: []].append(event)
            }
            let missingAttemptIDs = orderedIDs.filter { eventsByAttemptID[$0]?.isEmpty ?? true }
            let incompleteAttemptEvents: [[String: Any]] = orderedIDs.compactMap { attemptID in
                let eventTypes = Set(eventsByAttemptID[attemptID, default: []].compactMap { $0["event"] as? String })
                var missing = [String]()
                if !eventTypes.contains("decode_started") { missing.append("decode_started") }
                if eventTypes.isDisjoint(with: ["decode_success", "decode_failure", "live_frame_pending", "transfer_chunk",
                    "transfer_duplicate", "transfer_complete", "transfer_decode_failed", "transfer_packet_rejected"]) {
                    missing.append("decode_outcome")
                }
                return missing.isEmpty ? nil : ["attempt_id": attemptID, "missing_event_types": missing]
            }
            return [
                "camera_sampling": Self.cameraSamplingScope,
                "lens_position_scope": Self.lensPositionScope,
                "attempt_ids": orderedIDs,
                "events": matchedEvents,
                "missing_attempt_ids": missingAttemptIDs,
                "incomplete_attempt_events": incompleteAttemptEvents
            ]
        }
    }

    func flush() {
        queue.async { self.pendingWrite?.cancel(); self.persist() }
    }

    func export(capture: [String: Any]? = nil, completion: @escaping (Result<Data, Error>) -> Void) {
        queue.async {
            self.pendingWrite?.cancel()
            self.pendingWrite = nil
            let result = Result {
                _ = try self.writeSnapshot()
                return try self.snapshot(capture: capture)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func append(_ data: Data) {
        events.append(data)
        eventBytes += data.count
        while eventBytes > maximumBytes || events.count > 2_000 { eventBytes -= events.removeFirst().count }
    }

    func snapshot(capture: [String: Any]? = nil) throws -> Data {
        var metadata: [String: Any] = [
            "schema_version": 1,
            "app_version": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown",
            "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
            "camera_sampling": Self.cameraSamplingScope,
            "lens_position_scope": Self.lensPositionScope,
            "event_limit": 2_000, "event_bytes_limit": maximumBytes
        ]
        if let videoFormatInventory { metadata["video_format_inventory"] = videoFormatInventory }
        if let writeError { metadata["previous_persistence_error"] = writeError.localizedDescription }
        if let capture {
            metadata["captured_frame"] = capture
            metadata["capture_scope"] = capture["capture_kind"] as? String == "processed_still"
                ? "Diagnostics frozen for this still photo. The captured_frame entry contains this photo’s decode outcome; earlier attempts belong to other frames. The PNG contains the exact normalized BGRA pixels supplied to the decoder."
                : "Diagnostics frozen when this frame was captured. Earlier decode attempts belong to other frames; this captured frame was not decoded."
        }
        var data = try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
        data.removeLast()
        data.append(Data(",\"events\":[".utf8))
        for (index, event) in events.enumerated() {
            if index > 0 { data.append(44) }
            data.append(event)
        }
        data.append(Data("]}".utf8))
        return data
    }

    func writeSnapshot() throws -> Data {
        let data = try snapshot()
        try data.write(to: url, options: .atomic)
        return data
    }

    func persist() {
        pendingWrite = nil
        do { _ = try writeSnapshot(); writeError = nil }
        catch { writeError = error }
    }

}
