import CWCodec
import Foundation

final class RecoverySession {
    let handle: UnsafeMutableRawPointer
    let id = UUID()
    private(set) var captureIDs = [UUID]()

    init() throws {
        guard let handle = cw_recovery_create() else { throw CWFailure(message: "Cannot start photo recovery.") }
        self.handle = handle
    }

    deinit { cw_recovery_free(handle) }

    func decode(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int,
                captureID: UUID) throws -> PhotoRecoveryResult {
        let candidates = captureIDs + [captureID]
        let result = try Codec.decodeRecovery(pixels: pixels, length: length, width: width, height: height,
            stride: stride, session: handle, sessionID: id, captureIDs: candidates)
        if result.frames > captureIDs.count { captureIDs = candidates }
        return PhotoRecoveryResult(status: result.status, payload: result.payload, message: result.message,
            frames: result.frames, maxFrames: result.maxFrames, certifiedBlocks: result.certifiedBlocks,
            totalBlocks: result.totalBlocks, sessionID: result.sessionID,
            captureIDs: captureIDs.map { $0.uuidString.lowercased() }, codeDetected: result.codeDetected)
    }
}

final class TransferScannerSession {
    let handle: UnsafeMutableRawPointer

    init() throws {
        guard let handle = cw_transfer_scanner_create() else { throw CWFailure(message: "Cannot start transfer scanning.") }
        self.handle = handle
    }

    deinit { cw_transfer_scanner_free(handle) }

    func reset() { cw_transfer_scanner_reset(handle) }

    func decode(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int) throws -> DecodedPayload {
        try Codec.decodeTransfer(pixels: pixels, length: length, width: width, height: height,
            stride: stride, session: handle)
    }
}
