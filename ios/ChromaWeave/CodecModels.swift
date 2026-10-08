import Foundation
import UIKit

struct CWFailure: LocalizedError {
    let message: String
    var codeDetected = false
    var errorDescription: String? { message }
}

struct DecodedPayload: Identifiable {
    let id = UUID()
    let data: Data
    let profile: String
    let finalCallMilliseconds: Double
    var scanMilliseconds: Double? = nil
    let corrected: Int
    let erased: Int
    var transfer: TransferStatistics? = nil
    var text: String? { String(data: data, encoding: .utf8) }
    var profileName: String {
        switch profile {
        case "Luma10": "CW Luma10 · Standard error correction"
        case "Luma10LowEcc": "CW Luma10 · Low error correction"
        default: profile
        }
    }
}

struct TransferStatistics {
    let bytes: Int
    let elapsedSeconds: Double
    let chunks: Int
    let duplicates: Int
    let decodeFailures: Int
    var repairSymbols = 0
    var repairedChunks = 0
    var megabitsPerSecond: Double { Double(bytes) * 8 / max(elapsedSeconds, 0.000001) / 1_000_000 }
}

struct CapturedFrame: Identifiable {
    var id = UUID()
    let image: UIImage
    let width: Int
    let height: Int
    var diagnostics: Result<Data, Error>?
    var isPhoto = false
    var decodedPayload: DecodedPayload?
    var decodeError: String?
    var recoveryStatus: String?
    var recoveryMessage: String?
    var recoveryFrames: Int?
    var recoveryMaxFrames: Int?
    var recoveryCertifiedBlocks: Int?
    var recoveryTotalBlocks: Int?
    var recoverySessionID: String?
    var recoveryCaptureIDs: [String]?
    var captureDetails: String?
    var savedOnDevice = false
    var saveError: String?
    var exportStem: String { "cw-camera-frame-\(id.uuidString.lowercased())" }
}

struct PhotoRecoveryResult {
    let status: String
    let payload: DecodedPayload?
    let message: String?
    let frames: Int
    let maxFrames: Int
    let certifiedBlocks: Int?
    let totalBlocks: Int?
    let sessionID: String
    let captureIDs: [String]
    var codeDetected = false
}

struct EncodedCode {
    let png: Data
    let image: UIImage
    let side: Int
    let bytes: Int
    let storedBytes: Int
    let compression: String
}
