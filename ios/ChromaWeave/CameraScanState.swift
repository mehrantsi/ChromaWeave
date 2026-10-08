import AVFoundation
import UIKit

struct CameraVideoFrame: @unchecked Sendable {
    let pixels: CVPixelBuffer
    let receivedAt: CFTimeInterval
    let sensorTimestamp: Double
    let generation: Int
    let capture: Bool
    let lensRevision: Int
}

struct LiveDecodeTiming {
    let firstDetectedAt: CFTimeInterval
    let decodedAt: CFTimeInterval
    let captureSource: String
    let attemptID: String
}

struct LiveSheetTiming {
    let payloadID: UUID
    let generation: Int
    let firstDetectedAt: CFTimeInterval
    let decodedAt: CFTimeInterval
    let resultAssignedAt: CFTimeInterval
    let captureSource: String
    let attemptID: String
}


final class CameraScanState {
    let lock = NSLock()
    let configuration: CameraConfiguration
    var generation = 0
    var cameraWanted = false
    var captureRequested = false
    var automaticDecoding = true
    var transferDecoding = false
    var liveRecoveryActive = false
    var photoInFlight = false
    var cameraConfiguring = false
    var lensRevision = 0
    var pendingLiveSheetTiming: LiveSheetTiming?
    #if CW_DEVELOPMENT
    var liveTestResultTiming: LiveSheetTiming?
    #endif
    var thermalState = ProcessInfo.processInfo.thermalState.rawValue

    init(configuration: CameraConfiguration) { self.configuration = configuration }

    var isReplayingFrames: Bool { configuration == .savedFrames }
    var liveCaptureSource: String { isReplayingFrames ? "saved_bgra_provider" : "video_bgra" }
    var scanningStatus: String {
        lock.withLock {
            if transferDecoding { return "Waiting for CW transfer" }
            return automaticDecoding ? (liveRecoveryActive ? "Reading code…" : "Looking for CW Code") : "Photo mode · Ready to capture"
        }
    }

    func isCurrent(_ ticket: Int) -> Bool { lock.withLock { generation == ticket } }

    func invalidate(keepCameraWanted: Bool) -> Int? {
        lock.withLock {
            guard cameraWanted else { return nil }
            generation += 1
            cameraWanted = keepCameraWanted
            captureRequested = false
            photoInFlight = false
            cameraConfiguring = false
            liveRecoveryActive = false
            return generation
        }
    }
}

struct CameraPresentation {
    var status: String?
    var diagnostic: String?
    var focusStatus: String?
    var cameraAvailable: Bool?
    var processingImage: Bool?
    var liveCamera: Bool?
    var photoResolution: String?
    var photoAvailable: Bool?
    var mainCameraAvailable: Bool?
    var photoFocusGuidance: String?
}

enum CameraScanOutput {
    case presentation(CameraPresentation, ticket: Int)
    case invalidated
    case failure(Error, ticket: Int)
    case accepted(DecodedPayload, ticket: Int, timing: LiveDecodeTiming?)
    case captured(CapturedFrame, diagnostics: Result<Data, Error>, ticket: Int, savedPhoto: Bool)
    case photo(Result<(Data, [String: Any]), Error>, metadata: [String: Any], captureID: UUID, ticket: Int)
    case transfer(TransferReceiver.Update, startedAt: CFTimeInterval, validatedAt: CFTimeInterval, updateUI: Bool, ticket: Int)
    case transferFailure(detected: Bool, message: String, ticket: Int)
    case resetTransfer(ticket: Int)
}
