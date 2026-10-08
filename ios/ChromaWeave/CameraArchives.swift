import Foundation
import UIKit

struct LiveFailureFrame {
    let pixels: Data
    let metadata: [String: Any]
}

enum LiveFailureArchive {
    static func save(_ frames: [LiveFailureFrame], metadata: [String: Any], directory: URL? = nil) throws {
        let files = FileManager.default
        let directory = directory ?? files.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("LiveFailures", isDirectory: true)
        try files.createDirectory(at: directory, withIntermediateDirectories: true)
        let staging = directory.appendingPathComponent("staging", isDirectory: true)
        let latest = directory.appendingPathComponent("latest", isDirectory: true)
        if files.fileExists(atPath: staging.path) { try files.removeItem(at: staging) }
        try files.createDirectory(at: staging, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: staging) }
        var manifest = metadata
        var frameMetadata = [[String: Any]]()
        for (index, frame) in frames.enumerated() {
            let filename = "frame-\(index + 1).bgra"
            try frame.pixels.write(to: staging.appendingPathComponent(filename), options: .atomic)
            var fields = frame.metadata
            fields["filename"] = filename
            fields["byte_length"] = frame.pixels.count
            frameMetadata.append(fields)
        }
        manifest["frames"] = frameMetadata
        try JSONSerialization.data(withJSONObject: manifest, options: [.prettyPrinted, .sortedKeys])
            .write(to: staging.appendingPathComponent("diagnostics.json"), options: .atomic)
        if files.fileExists(atPath: latest.path) { try files.removeItem(at: latest) }
        try files.moveItem(at: staging, to: latest)
    }
}

enum CapturedPhotoArchive {
    static func save(_ frame: CapturedFrame, directory: URL = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("Captures", isDirectory: true)) throws {
        guard frame.isPhoto, let diagnostics = frame.diagnostics else { throw CWFailure(message: "No matching photo diagnostics are available to save.") }
        let snapshot = try diagnostics.get()
        guard let png = frame.image.pngData() else { throw CWFailure(message: "Cannot prepare the photo as PNG.") }
        let files = FileManager.default
        try files.createDirectory(at: directory, withIntermediateDirectories: true)
        let staging = directory.appendingPathComponent(".\(UUID().uuidString).tmp", isDirectory: true)
        let destination = directory.appendingPathComponent(frame.id.uuidString.lowercased(), isDirectory: true)
        try files.createDirectory(at: staging, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: staging) }
        try png.write(to: staging.appendingPathComponent("\(frame.exportStem).png"), options: .atomic)
        try snapshot.write(to: staging.appendingPathComponent("\(frame.exportStem)-diagnostics.json"), options: .atomic)
        try files.moveItem(at: staging, to: destination)
    }

}
