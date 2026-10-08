import Foundation

enum AppOwnedDiagnosticCleanup {
    struct Report {
        let didRun: Bool
        let removedPaths: [String]
        let failures: [String]
        var succeeded: Bool { failures.isEmpty }
    }

    private static let directories = ["Captures", "LiveFailures", "TransferSamples"]
    private static let diagnosticFiles = [
        "scan-diagnostics.json", "camera-video-formats.json", "completed-transfers.json",
        "photo-selftest.json", "camera-regression-result.json", "transfer-selftest-result.json", "transfer-repair-selftest-result.json",
        "live-stream-selftest-result.json", "live-recovery-regression-result.json",
        "recovery-regression-result.json", "diagnostics-selftest.json", "selftest.json"
    ]

    static func reset(documents: URL, fileManager: FileManager = .default,
                      removeItem: ((URL) throws -> Void)? = nil) -> Report {
        let remove = removeItem ?? { try fileManager.removeItem(at: $0) }
        var removedPaths = [String]()
        var failures = [String]()
        for path in directories + diagnosticFiles {
            let url = documents.appendingPathComponent(path)
            guard fileManager.fileExists(atPath: url.path) else { continue }
            do {
                try remove(url)
                removedPaths.append(path)
            } catch { failures.append("\(path): \(error.localizedDescription)") }
        }
        return Report(didRun: true, removedPaths: removedPaths, failures: failures)
    }
}
