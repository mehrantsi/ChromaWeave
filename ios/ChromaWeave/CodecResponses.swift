import CWCodec
import Foundation

struct CodecResponse: Decodable {
    enum Status: String, Decodable { case decoded, pending, error, notFound = "not_found", encoded }
    let status: Status
    let message: String?
    let codeDetected: Bool?
    let payload: [UInt8]?
    let modulation: String?
    let elapsedMs: Double?
    let correctedBytes: Int?
    let erasedBytes: Int?
    let frames: Int?
    let maxFrames: Int?
    let certifiedBlocks: Int?
    let totalBlocks: Int?
    let png: [UInt8]?
    let sidePixels: Int?
    let encodedPayloadBytes: Int?
    let compression: String?

    func decodedPayload() throws -> DecodedPayload {
        guard status == .decoded else {
            throw CWFailure(message: message ?? "No CW code found.", codeDetected: codeDetected ?? false)
        }
        guard let payload, let modulation, let elapsedMs, let correctedBytes, let erasedBytes,
              elapsedMs.isFinite, elapsedMs >= 0, correctedBytes >= 0, erasedBytes >= 0 else {
            throw CWFailure(message: "Incomplete decoded codec response.")
        }
        return DecodedPayload(data: Data(payload), profile: modulation, finalCallMilliseconds: elapsedMs,
            corrected: correctedBytes, erased: erasedBytes)
    }

    func recovery(sessionID: UUID, captureIDs: [UUID]) throws -> PhotoRecoveryResult {
        guard [.decoded, .pending, .error].contains(status),
              let frames, let maxFrames, frames >= 0, maxFrames > 0, frames <= maxFrames,
              frames <= captureIDs.count else {
            throw CWFailure(message: message ?? "Invalid recovery response.")
        }
        return PhotoRecoveryResult(status: status.rawValue,
            payload: status == .decoded ? try decodedPayload() : nil,
            message: message, frames: frames, maxFrames: maxFrames,
            certifiedBlocks: certifiedBlocks, totalBlocks: totalBlocks,
            sessionID: sessionID.uuidString.lowercased(),
            captureIDs: captureIDs.prefix(frames).map { $0.uuidString.lowercased() },
            codeDetected: codeDetected ?? (status == .pending || status == .decoded))
    }
}

struct TransferRepairResponse: Decodable {
    struct Shard: Decodable {
        let index: Int
        let dataHex: String
    }
    let ok: Bool
    let message: String?
    let recovered: [Shard]?
}

extension Codec {
    static func parseResponse<T: Decodable>(_ data: Data, as type: T.Type) throws -> T {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do { return try decoder.decode(type, from: data) }
        catch { throw CWFailure(message: "Invalid codec response: \(error.localizedDescription)") }
    }

    static func response<T: Decodable>(_ pointer: UnsafeMutablePointer<CChar>?, as type: T.Type) throws -> T {
        guard let pointer else { throw CWFailure(message: "The codec returned no result.") }
        defer { cw_string_free(pointer) }
        return try parseResponse(Data(String(cString: pointer).utf8), as: type)
    }
}
