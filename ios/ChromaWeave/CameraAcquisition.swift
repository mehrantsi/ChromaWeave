import AVFoundation
import UIKit
import ImageIO

final class CameraAcquisition: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    let session = AVCaptureSession()
    let sessionQueue = DispatchQueue(label: "cw.camera.session")
    let captureQueue = DispatchQueue(label: "cw.camera.capture", qos: .userInitiated)
    private let photoOutput = AVCapturePhotoOutput()
    private var photoCaptures = [Int64: StillPhotoCapture]()
    private var configured = false
    private var didRecordVideoFormatInventory = false
    private var configuredPhotoMode = false
    private var configuredPhotoCamera = PhotoCamera.automatic
    private var requestedPhotoCamera = PhotoCamera.automatic
    private var automaticCamera: AVCaptureDevice?
    private var mainCamera: AVCaptureDevice?
    private var cameraInput: AVCaptureDeviceInput?
    private var camera: AVCaptureDevice?
    private var focusObservation: NSKeyValueObservation?
    private var adjustingFocusObservation: NSKeyValueObservation?
    private var lensObservation: NSKeyValueObservation?
    private var focusRequest = 0
    private var cameraDiagnosticSnapshot = [String: Any]()
    private var cameraDiagnosticSnapshotAt = 0.0

    private let state: CameraScanState
    let diagnostics: ScanDiagnosticStore
    private let output: (CameraScanOutput) -> Void
    private let frameHandler: (CMSampleBuffer) -> Void
    private let droppedFrameHandler: () -> Void

    init(state: CameraScanState, diagnostics: ScanDiagnosticStore, output: @escaping (CameraScanOutput) -> Void,
         frameHandler: @escaping (CMSampleBuffer) -> Void, droppedFrameHandler: @escaping () -> Void) {
        self.state = state
        self.diagnostics = diagnostics
        self.output = output
        self.frameHandler = frameHandler
        self.droppedFrameHandler = droppedFrameHandler
        super.init()
        for name in [AVCaptureSession.runtimeErrorNotification, AVCaptureSession.wasInterruptedNotification,
                     AVCaptureSession.interruptionEndedNotification, AVCaptureSession.didStartRunningNotification,
                     AVCaptureSession.didStopRunningNotification] {
            NotificationCenter.default.addObserver(self, selector: #selector(sessionStateChanged(_:)), name: name, object: session)
        }
        NotificationCenter.default.addObserver(self, selector: #selector(thermalStateChanged),
            name: ProcessInfo.thermalStateDidChangeNotification, object: nil)
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        frameHandler(sampleBuffer)
    }

    func captureOutput(_ output: AVCaptureOutput, didDrop sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        droppedFrameHandler()
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        let captureSession = session
        sessionQueue.async { if captureSession.isRunning { captureSession.stopRunning() } }
    }

    static func interruptionMessage(_ reason: Int?) -> String {
        switch reason.flatMap(AVCaptureSession.InterruptionReason.init(rawValue:)) {
        case .videoDeviceNotAvailableInBackground: "Camera unavailable while the app is in the background."
        case .videoDeviceInUseByAnotherClient: "Another app or system service is using the camera."
        case .audioDeviceInUseByAnotherClient: "Capture interrupted by another app’s audio session."
        case .videoDeviceNotAvailableWithMultipleForegroundApps: "Camera unavailable while multiple apps share the foreground."
        case .videoDeviceNotAvailableDueToSystemPressure: "Camera paused by system pressure."
        default: "Camera unavailable during a system interruption."
        }
    }

    static func configureLiveVideo(_ device: AVCaptureDevice) throws {
        let subtype = CMFormatDescriptionGetMediaSubType(device.activeFormat.formatDescription)
        let framesPerSecond: Int32 = 60
        let frameDuration = CMTime(value: 1, timescale: framesPerSecond)
        let formats = device.formats.filter { format in
            let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
            return CMFormatDescriptionGetMediaType(format.formatDescription) == kCMMediaType_Video
                && Int64(dimensions.width) * Int64(dimensions.height) <= 40_000_000
                && format.supportedColorSpaces.contains(.sRGB)
                && !format.isVideoBinned
                && format.videoSupportedFrameRateRanges.contains {
                    $0.minFrameRate <= Double(framesPerSecond) && $0.maxFrameRate >= Double(framesPerSecond)
                }
        }
        guard let format = formats.max(by: { lhs, rhs in
            let a = CMVideoFormatDescriptionGetDimensions(lhs.formatDescription)
            let b = CMVideoFormatDescriptionGetDimensions(rhs.formatDescription)
            let shortA = min(a.width, a.height), shortB = min(b.width, b.height)
            if shortA != shortB { return shortA < shortB }
            let longA = max(a.width, a.height), longB = max(b.width, b.height)
            if longA != longB { return longA < longB }
            return CMFormatDescriptionGetMediaSubType(lhs.formatDescription) != subtype
                && CMFormatDescriptionGetMediaSubType(rhs.formatDescription) == subtype
        }) else {
            throw CWFailure(message: "This camera does not support unbinned sRGB capture at 60 FPS.")
        }
        device.activeFormat = format
        device.activeVideoMinFrameDuration = frameDuration
        device.activeVideoMaxFrameDuration = frameDuration
        guard device.activeFormat.supportedColorSpaces.contains(.sRGB) else {
            throw CWFailure(message: "The selected Live camera format does not support sRGB.")
        }
        device.activeColorSpace = .sRGB
        device.automaticallyAdjustsVideoHDREnabled = false
        device.isVideoHDREnabled = false
    }

    static func configureExposure(_ device: AVCaptureDevice, photoMode: Bool) {
        guard device.isExposureModeSupported(.continuousAutoExposure) else { return }
        device.exposureMode = .continuousAutoExposure
        device.activeMaxExposureDuration = photoMode ? .invalid : CMTimeMaximum(
            device.activeFormat.minExposureDuration,
            CMTimeMinimum(CMTime(value: 1, timescale: 240), device.activeFormat.maxExposureDuration)
        )
    }

    static func nativeWideZoom(_ device: AVCaptureDevice) -> CGFloat {
        device.virtualDeviceSwitchOverVideoZoomFactors.first.map { CGFloat(truncating: $0) } ?? 1
    }

    static func photoFormat(_ device: AVCaptureDevice) -> AVCaptureDevice.Format? {
        device.formats.filter { $0.isHighestPhotoQualitySupported && photoArea($0) > 0 }.max {
            photoArea($0) < photoArea($1)
        }
    }

    static func photoArea(_ format: AVCaptureDevice.Format) -> Int64 {
        format.supportedMaxPhotoDimensions.map { Int64($0.width) * Int64($0.height) }.max() ?? 0
    }

    static func maximumPhotoDimensions(_ formats: [AVCaptureDevice.Format]) -> [String: Int32]? {
        guard let dimensions = formats.flatMap(\.supportedMaxPhotoDimensions).max(by: {
            Int64($0.width) * Int64($0.height) < Int64($1.width) * Int64($1.height)
        }) else { return nil }
        return ["width": dimensions.width, "height": dimensions.height]
    }

    static func videoFormatInventoryFields(
        automaticDevice: AVCaptureDevice,
        mainDevice: AVCaptureDevice?,
        session: AVCaptureSession,
        output: AVCaptureVideoDataOutput
    ) -> [String: Any] {
        let maximumDevices = 8
        let maximumFormatsPerDevice = 48
        var orderedIDs = [String]()
        var devicesByID = [String: AVCaptureDevice]()
        var rolesByID = [String: Set<String>]()
        func include(_ device: AVCaptureDevice, role: String) {
            if devicesByID[device.uniqueID] == nil {
                orderedIDs.append(device.uniqueID)
                devicesByID[device.uniqueID] = device
            }
            rolesByID[device.uniqueID, default: []].insert(role)
        }
        include(automaticDevice, role: "automatic_capture_device")
        for device in automaticDevice.constituentDevices {
            include(device, role: "automatic_constituent_physical")
        }
        if let mainDevice { include(mainDevice, role: "main_wide_camera") }

        let recordedIDs = Array(orderedIDs.prefix(maximumDevices))
        let inventoryDevices: [[String: Any]] = recordedIDs.compactMap { uniqueID in
            guard let device = devicesByID[uniqueID] else { return nil }
            let candidateFormats = device.formats.filter { format in
                guard format.mediaType == .video else { return false }
                let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
                return dimensions.width >= 1920 && dimensions.height >= 1080
            }.sorted { left, right in
                let leftDimensions = CMVideoFormatDescriptionGetDimensions(left.formatDescription)
                let rightDimensions = CMVideoFormatDescriptionGetDimensions(right.formatDescription)
                let leftArea = Int64(leftDimensions.width) * Int64(leftDimensions.height)
                let rightArea = Int64(rightDimensions.width) * Int64(rightDimensions.height)
                if leftArea != rightArea { return leftArea > rightArea }
                let leftMaximumFPS = left.videoSupportedFrameRateRanges.map(\.maxFrameRate).max() ?? 0
                let rightMaximumFPS = right.videoSupportedFrameRateRanges.map(\.maxFrameRate).max() ?? 0
                if leftMaximumFPS != rightMaximumFPS { return leftMaximumFPS > rightMaximumFPS }
                return CMFormatDescriptionGetMediaSubType(left.formatDescription)
                    < CMFormatDescriptionGetMediaSubType(right.formatDescription)
            }
            var distinctFormats = [AVCaptureDevice.Format]()
            var duplicateFormatCounts = [Int]()
            var indexByCapability = [String: Int]()
            for format in candidateFormats {
                let key = Self.videoFormatCapabilityKey(format)
                if let index = indexByCapability[key] {
                    duplicateFormatCounts[index] += 1
                } else {
                    indexByCapability[key] = distinctFormats.count
                    distinctFormats.append(format)
                    duplicateFormatCounts.append(1)
                }
            }
            let formats = Array(zip(distinctFormats, duplicateFormatCounts).prefix(maximumFormatsPerDevice)).map { pair -> [String: Any] in
                let format = pair.0
                let duplicateCount = pair.1
                let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
                let ranges = format.videoSupportedFrameRateRanges.map { range in
                    ["minimum_fps": range.minFrameRate, "maximum_fps": range.maxFrameRate]
                }
                var summary: [String: Any] = [
                    "width": Int(dimensions.width),
                    "height": Int(dimensions.height),
                    "aspect_ratio": Double(dimensions.width) / Double(dimensions.height),
                    "media_subtype": Self.mediaSubtypeString(CMFormatDescriptionGetMediaSubType(format.formatDescription)),
                    "media_subtype_code": Int(CMFormatDescriptionGetMediaSubType(format.formatDescription)),
                    "horizontal_video_field_of_view_degrees": Double(format.videoFieldOfView),
                    "minimum_exposure_duration_seconds": CMTimeGetSeconds(format.minExposureDuration),
                    "maximum_exposure_duration_seconds": CMTimeGetSeconds(format.maxExposureDuration),
                    "supported_frame_rate_ranges": ranges,
                    "supports_srgb": format.supportedColorSpaces.contains(.sRGB),
                    "is_video_binned": format.isVideoBinned,
                    "video_zoom_factor_upscale_threshold": Double(format.videoZoomFactorUpscaleThreshold),
                    "identical_capability_format_count": duplicateCount
                ]
                if let minimumFPS = format.videoSupportedFrameRateRanges.map(\.minFrameRate).min() {
                    summary["minimum_supported_fps"] = minimumFPS
                }
                if let maximumFPS = format.videoSupportedFrameRateRanges.map(\.maxFrameRate).max() {
                    summary["maximum_supported_fps"] = maximumFPS
                }
                return summary
            }
            let activeDimensions = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
            var entry: [String: Any] = [
                "device_unique_id": device.uniqueID,
                "device_type": device.deviceType.rawValue,
                "roles": (rolesByID[uniqueID] ?? []).sorted(),
                "is_virtual_device": device.isVirtualDevice,
                "constituent_physical_device_ids": device.constituentDevices.map(\.uniqueID),
                "virtual_device_switch_over_zoom_factors": device.virtualDeviceSwitchOverVideoZoomFactors.map(\.doubleValue),
                "minimum_focus_distance_mm": device.minimumFocusDistance,
                "active_format_width": Int(activeDimensions.width),
                "active_format_height": Int(activeDimensions.height),
                "active_video_zoom_factor": device.videoZoomFactor,
                "active_video_zoom_factor_upscale_threshold": Double(device.activeFormat.videoZoomFactorUpscaleThreshold),
                "active_format_is_video_binned": device.activeFormat.isVideoBinned,
                "active_max_exposure_duration_seconds": CMTimeGetSeconds(device.activeMaxExposureDuration),
                "candidate_format_count": candidateFormats.count,
                "distinct_candidate_format_count": distinctFormats.count,
                "duplicate_candidate_format_count": candidateFormats.count - distinctFormats.count,
                "recorded_format_count": formats.count,
                "candidate_formats_truncated": distinctFormats.count > formats.count,
                "formats": formats
            ]
            entry["active_format_media_subtype"] = Self.mediaSubtypeString(
                CMFormatDescriptionGetMediaSubType(device.activeFormat.formatDescription)
            )
            return entry
        }

        let candidateFormatCount = inventoryDevices.reduce(0) {
            $0 + ($1["candidate_format_count"] as? Int ?? 0)
        }
        let distinctCandidateFormatCount = inventoryDevices.reduce(0) {
            $0 + ($1["distinct_candidate_format_count"] as? Int ?? 0)
        }
        let recordedFormatCount = inventoryDevices.reduce(0) {
            $0 + ($1["recorded_format_count"] as? Int ?? 0)
        }
        let candidateFormatsTruncated = inventoryDevices.contains {
            $0["candidate_formats_truncated"] as? Bool == true
        }

        let settings = output.videoSettings
        let width = (settings?[kCVPixelBufferWidthKey as String] as? NSNumber)?.intValue
        let height = (settings?[kCVPixelBufferHeightKey as String] as? NSNumber)?.intValue
        var outputFields: [String: Any] = [
            "automatically_configures_output_buffer_dimensions": output.automaticallyConfiguresOutputBufferDimensions,
            "delivers_preview_sized_output_buffers": output.deliversPreviewSizedOutputBuffers,
            "settings_dimensions_present": width != nil && height != nil,
            "session_preset": session.sessionPreset.rawValue
        ]
        if let width { outputFields["settings_width"] = width }
        if let height { outputFields["settings_height"] = height }
        if let pixelFormat = (settings?[kCVPixelBufferPixelFormatTypeKey as String] as? NSNumber)?.uint32Value {
            outputFields["pixel_format"] = Self.mediaSubtypeString(pixelFormat)
            outputFields["pixel_format_code"] = Int(pixelFormat)
        }
        var inventory: [String: Any] = [
            "inventory_device_count": orderedIDs.count,
            "recorded_device_count": inventoryDevices.count,
            "inventory_devices_truncated": orderedIDs.count > inventoryDevices.count,
            "minimum_video_width": 1920,
            "minimum_video_height": 1080,
            "maximum_formats_per_device": maximumFormatsPerDevice,
            "candidate_format_count": candidateFormatCount,
            "distinct_candidate_format_count": distinctCandidateFormatCount,
            "recorded_format_count": recordedFormatCount,
            "candidate_formats_truncated": candidateFormatsTruncated,
            "automatic_device_id": automaticDevice.uniqueID,
            "automatic_active_primary_device_id": automaticDevice.activePrimaryConstituent?.uniqueID ?? automaticDevice.uniqueID,
            "automatic_constituent_physical_device_ids": automaticDevice.constituentDevices.map(\.uniqueID),
            "devices": inventoryDevices,
            "video_output": outputFields
        ]
        if let mainDevice { inventory["main_device_id"] = mainDevice.uniqueID }
        return inventory
    }

    static func videoFormatCapabilityKey(_ format: AVCaptureDevice.Format) -> String {
        let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
        let ranges = format.videoSupportedFrameRateRanges
            .map { "\($0.minFrameRate.bitPattern):\($0.maxFrameRate.bitPattern)" }
            .sorted()
            .joined(separator: ",")
        let subtype = CMFormatDescriptionGetMediaSubType(format.formatDescription)
        let supportsSRGB = format.supportedColorSpaces.contains(.sRGB)
        let fieldOfView = Double(format.videoFieldOfView).bitPattern
        let minimumExposure = CMTimeGetSeconds(format.minExposureDuration).bitPattern
        let maximumExposure = CMTimeGetSeconds(format.maxExposureDuration).bitPattern
        let upscaleThreshold = Double(format.videoZoomFactorUpscaleThreshold).bitPattern
        return [
            String(dimensions.width),
            String(dimensions.height),
            String(subtype),
            ranges,
            String(fieldOfView),
            String(minimumExposure),
            String(maximumExposure),
            String(describing: supportsSRGB),
            String(describing: format.isVideoBinned),
            String(upscaleThreshold)
        ].joined(separator: "|")
    }

    static func mediaSubtypeString(_ subtype: UInt32) -> String {
        let bytes = [
            UInt8((subtype >> 24) & 0xff),
            UInt8((subtype >> 16) & 0xff),
            UInt8((subtype >> 8) & 0xff),
            UInt8(subtype & 0xff)
        ]
        return String(bytes: bytes, encoding: .ascii) ?? "0x\(String(subtype, radix: 16))"
    }
}

extension CameraAcquisition {
    @objc func sessionStateChanged(_ notification: Notification) {
        guard !state.isReplayingFrames else { return }
        let error = notification.userInfo?[AVCaptureSessionErrorKey] as? NSError
        let reason = (notification.userInfo?[AVCaptureSessionInterruptionReasonKey] as? NSNumber)?.intValue
        sessionQueue.async { [weak self] in
            guard let self else { return }
            let event: String
            switch notification.name {
            case AVCaptureSession.runtimeErrorNotification: event = "session_runtime_error"
            case AVCaptureSession.wasInterruptedNotification: event = "session_interrupted"
            case AVCaptureSession.interruptionEndedNotification: event = "session_interruption_ended"
            case AVCaptureSession.didStartRunningNotification: event = "session_started"
            default: event = "session_stopped"
            }
            var fields: [String: Any] = [:]
            if let reason {
                fields["interruption_reason"] = reason
                fields["interruption_message"] = CameraAcquisition.interruptionMessage(reason)
            }
            if let error {
                fields["error_domain"] = error.domain
                fields["error_code"] = error.code
                fields["error"] = error.localizedDescription
            }
            self.recordCameraEvent(event, fields: fields)
            let runtimeError = notification.name == AVCaptureSession.runtimeErrorNotification
            let interrupted = notification.name == AVCaptureSession.wasInterruptedNotification || self.session.isInterrupted
            let ready = self.session.isRunning && !interrupted && !runtimeError
            let ended = notification.name == AVCaptureSession.interruptionEndedNotification
            let invalidated = runtimeError || interrupted || !ready && !ended
            let ticket: Int
            if invalidated {
                guard let current = self.invalidate(keepCameraWanted: interrupted && !runtimeError) else { return }
                ticket = current
            } else {
                guard let current = self.state.lock.withLock({ () -> Int? in
                    guard self.state.cameraWanted else { return nil }
                    if ended { self.state.cameraConfiguring = true }
                    return self.state.generation
                }) else { return }
                ticket = current
            }
            let message = runtimeError ? "Camera error · Resume to try again" :
                (interrupted ? "Camera interrupted" : (ready ? nil : "Camera stopped · Resume to try again"))
            DispatchQueue.main.async {
                guard self.state.isCurrent(ticket) else { return }
                self.output(.presentation(CameraPresentation(
                    status: ended ? "Resuming camera…" : (message ?? self.state.scanningStatus),
                    diagnostic: error?.localizedDescription ?? (interrupted ? CameraAcquisition.interruptionMessage(reason) : nil),
                    processingImage: !invalidated && self.state.lock.withLock { self.state.automaticDecoding && self.state.liveRecoveryActive },
                    liveCamera: ready), ticket: ticket))
            }
            if ended { self.configureAndStart() }
            else if runtimeError || !interrupted && !ready { self.stopSession() }
        }
    }

    func focus(at point: CGPoint) {
        sessionQueue.async { [weak self] in
            guard let self, self.state.lock.withLock({ self.state.cameraWanted }), let device = self.camera else { return }
            let point = CGPoint(x: min(max(point.x, 0), 1), y: min(max(point.y, 0), 1))
            do {
                self.cancelFocusRequest()
                try device.lockForConfiguration()
                defer { device.unlockForConfiguration() }
                if device.isFocusPointOfInterestSupported { device.focusPointOfInterest = point }
                if device.isFocusModeSupported(.autoFocus), device.isFocusModeSupported(.continuousAutoFocus) {
                    let request = self.focusRequest
                    self.focusObservation = device.observe(\.isAdjustingFocus, options: [.old, .new]) { [weak self] _, change in
                        guard change.oldValue == true, change.newValue == false else { return }
                        self?.sessionQueue.async { [weak self] in
                            guard let self, request == self.focusRequest,
                                  self.state.lock.withLock({ self.state.cameraWanted }) else { return }
                            self.continuousFocus(at: point, reason: "focus_completed", request: request)
                        }
                    }
                    device.focusMode = .autoFocus
                    self.publishFocusStatus("Focusing…")
                } else if device.isFocusModeSupported(.continuousAutoFocus) {
                    device.focusMode = .continuousAutoFocus
                    self.publishFocusStatus("Continuous focus")
                } else {
                    self.publishFocusStatus("Autofocus unavailable")
                }
                self.recordCameraEvent("focus_requested", fields: ["x": point.x, "y": point.y, "focus_request_id": self.focusRequest])
            } catch { self.focusFailed(error) }
        }
    }

    func refocus() { focus(at: CGPoint(x: 0.5, y: 0.5)) }

    func cancelFocusRequest() {
        focusRequest += 1
        focusObservation = nil
    }

    func continuousFocus(at point: CGPoint = CGPoint(x: 0.5, y: 0.5), reason: String, request: Int? = nil) {
        guard let device = camera else { return }
        cancelFocusRequest()
        do {
            try device.lockForConfiguration()
            defer { device.unlockForConfiguration() }
            if device.isFocusPointOfInterestSupported { device.focusPointOfInterest = point }
            if device.isFocusModeSupported(.continuousAutoFocus) {
                device.focusMode = .continuousAutoFocus
                publishFocusStatus(device.isAdjustingFocus ? "Focusing…" : "Continuous focus")
            } else { publishFocusStatus("Autofocus unavailable") }
            var fields: [String: Any] = [:]
            if let request {
                fields["focus_request_id"] = request
                fields["completion_signal"] = "adjusting_focus_true_to_false"
            }
            recordCameraEvent(reason, fields: fields)
        } catch { focusFailed(error) }
    }

    func publishFocusStatus(_ value: String) {
        let ticket = state.lock.withLock { state.generation }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.state.isCurrent(ticket) else { return }
            self.output(.presentation(CameraPresentation(focusStatus: value), ticket: ticket))
        }
    }

    func focusFailed(_ error: Error) {
        cancelFocusRequest()
        publishFocusStatus("Could not adjust focus")
        recordCameraEvent("focus_error", fields: ["error": error.localizedDescription])
    }

    @objc func subjectAreaChanged(_ notification: Notification) {
        sessionQueue.async { [weak self] in
            guard let self, self.state.lock.withLock({ self.state.cameraWanted }), self.focusObservation == nil else { return }
            self.continuousFocus(reason: "subject_area_changed")
        }
    }

    @objc func thermalStateChanged() {
        state.lock.withLock { state.thermalState = ProcessInfo.processInfo.thermalState.rawValue }
        recordCameraEvent("thermal_state_changed")
    }

    func stopSession() {
        sessionQueue.async { [weak self] in
            guard let self, !self.state.lock.withLock({ self.state.cameraWanted }) else { return }
            self.cancelFocusRequest()
            if self.session.isRunning { self.session.stopRunning() }
            self.diagnostics.flush()
        }
    }

    private func invalidate(keepCameraWanted: Bool) -> Int? {
        let ticket = state.invalidate(keepCameraWanted: keepCameraWanted)
        if ticket != nil { output(.invalidated) }
        return ticket
    }

    func unavailable(_ message: String) {
        guard let ticket = invalidate(keepCameraWanted: false) else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.state.isCurrent(ticket) else { return }
            self.output(.presentation(CameraPresentation(status: message, cameraAvailable: false,
                processingImage: false, liveCamera: false), ticket: ticket))
        }
        stopSession()
    }

    func configureAndStart() {
        sessionQueue.async { [weak self] in
            guard let self, state.lock.withLock({ self.state.cameraWanted }) else { return }
            let ticket = state.lock.withLock { self.state.cameraConfiguring = true; return self.state.generation }
            if !configured {
                let cameras = AVCaptureDevice.DiscoverySession(
                    deviceTypes: [.builtInTripleCamera, .builtInDualWideCamera, .builtInWideAngleCamera],
                    mediaType: .video, position: .back
                )
                guard let device = cameras.devices.first,
                      let input = try? AVCaptureDeviceInput(device: device) else {
                    unavailable("No camera is available here. Import an image or try the sample.")
                    return
                }
                automaticCamera = device
                mainCamera = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
                session.beginConfiguration()
                session.automaticallyConfiguresCaptureDeviceForWideColor = false
                let output = AVCaptureVideoDataOutput()
                output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
                output.automaticallyConfiguresOutputBufferDimensions = false
                output.deliversPreviewSizedOutputBuffers = false
                output.alwaysDiscardsLateVideoFrames = true
                output.setSampleBufferDelegate(self, queue: captureQueue)
                guard session.canAddInput(input), session.canAddOutput(output) else {
                    session.commitConfiguration()
                    unavailable("Cannot start this camera.")
                    return
                }
                session.addInput(input)
                session.addOutput(output)
                if session.canSetSessionPreset(.hd4K3840x2160) {
                    session.sessionPreset = .hd4K3840x2160
                } else if session.canSetSessionPreset(.hd1920x1080) {
                    session.sessionPreset = .hd1920x1080
                }
                do {
                    try device.lockForConfiguration()
                    defer { device.unlockForConfiguration() }
                    try CameraAcquisition.configureLiveVideo(device)
                    if device.primaryConstituentDeviceSwitchingBehavior != .unsupported {
                        device.setPrimaryConstituentDeviceSwitchingBehavior(.auto, restrictedSwitchingBehaviorConditions: [])
                    }
                    device.videoZoomFactor = min(max(CameraAcquisition.nativeWideZoom(device), device.minAvailableVideoZoomFactor),
                                                 device.maxAvailableVideoZoomFactor)
                    if device.isFocusModeSupported(.continuousAutoFocus) { device.focusMode = .continuousAutoFocus }
                    CameraAcquisition.configureExposure(device, photoMode: false)
                    device.isSubjectAreaChangeMonitoringEnabled = true
                } catch {
                    session.removeOutput(output)
                    session.removeInput(input)
                    session.commitConfiguration()
                    unavailable("Cannot configure camera: \(error.localizedDescription)")
                    return
                }
                if session.canAddOutput(photoOutput) {
                    session.addOutput(photoOutput)
                    photoOutput.maxPhotoQualityPrioritization = .quality
                    if let largest = device.activeFormat.supportedMaxPhotoDimensions.max(by: {
                        Int64($0.width) * Int64($0.height) < Int64($1.width) * Int64($1.height)
                    }) { photoOutput.maxPhotoDimensions = largest }
                }
                session.commitConfiguration()
                cameraInput = input
                observeCamera(device)
                configured = true
            }
            guard state.lock.withLock({ self.state.cameraWanted && self.state.generation == ticket }) else { return }
            let (photoMode, photoCamera) = state.lock.withLock {
                (!self.state.automaticDecoding, self.state.automaticDecoding ? PhotoCamera.main : self.requestedPhotoCamera)
            }
            if configuredPhotoMode != photoMode || configuredPhotoCamera != photoCamera {
                guard let device = photoCamera == .main ? mainCamera : automaticCamera,
                      let preset = capturePreset(photoMode: photoMode) else {
                    state.lock.withLock { self.state.cameraWanted = false; self.state.cameraConfiguring = false }
                    output(.failure(CWFailure(message: "This camera does not support the requested capture mode."), ticket: ticket))
                    stopSession()
                    return
                }
                cancelFocusRequest()
                let previousInput = cameraInput
                var replacementInput: AVCaptureDeviceInput?
                let desiredZoom = CameraAcquisition.nativeWideZoom(device)
                session.beginConfiguration()
                session.automaticallyConfiguresCaptureDeviceForWideColor = photoMode
                do {
                    if camera?.uniqueID != device.uniqueID {
                        let input = try AVCaptureDeviceInput(device: device)
                        if let previousInput { session.removeInput(previousInput) }
                        guard session.canAddInput(input) else { throw CWFailure(message: "Cannot select this camera.") }
                        session.addInput(input)
                        replacementInput = input
                    }
                    session.sessionPreset = preset
                    try device.lockForConfiguration()
                    defer { device.unlockForConfiguration() }
                    if photoMode, let bestFormat = CameraAcquisition.photoFormat(device),
                       photoCamera == .main || CameraAcquisition.photoArea(bestFormat) > CameraAcquisition.photoArea(device.activeFormat) {
                        device.activeFormat = bestFormat
                    }
                    if photoMode {
                        device.automaticallyAdjustsVideoHDREnabled = true
                    } else {
                        try CameraAcquisition.configureLiveVideo(device)
                    }
                    if let output = session.outputs.compactMap({ $0 as? AVCaptureVideoDataOutput }).first {
                        output.automaticallyConfiguresOutputBufferDimensions = photoMode
                        if !photoMode { output.deliversPreviewSizedOutputBuffers = false }
                    }
                    if device.primaryConstituentDeviceSwitchingBehavior != .unsupported {
                        device.setPrimaryConstituentDeviceSwitchingBehavior(.auto, restrictedSwitchingBehaviorConditions: [])
                    }
                    device.videoZoomFactor = min(max(desiredZoom, device.minAvailableVideoZoomFactor), device.maxAvailableVideoZoomFactor)
                    if device.isFocusModeSupported(.continuousAutoFocus) { device.focusMode = .continuousAutoFocus }
                    CameraAcquisition.configureExposure(device, photoMode: photoMode)
                    device.isSubjectAreaChangeMonitoringEnabled = true
                    if session.outputs.contains(photoOutput), let dimensions = device.activeFormat.supportedMaxPhotoDimensions.max(by: {
                        Int64($0.width) * Int64($0.height) < Int64($1.width) * Int64($1.height)
                    }) {
                        photoOutput.maxPhotoDimensions = dimensions
                        photoOutput.maxPhotoQualityPrioritization = .quality
                    }
                } catch {
                    if let replacementInput { session.removeInput(replacementInput) }
                    if let previousInput, !session.inputs.contains(previousInput) { session.addInput(previousInput) }
                    session.commitConfiguration()
                    state.lock.withLock { self.state.cameraWanted = false; self.state.cameraConfiguring = false }
                    output(.failure(CWFailure(message: "Cannot configure camera: \(error.localizedDescription)"), ticket: ticket))
                    stopSession()
                    return
                }
                session.commitConfiguration()
                if let replacementInput {
                    cameraInput = replacementInput
                    observeCamera(device)
                }
                configuredPhotoMode = photoMode
                configuredPhotoCamera = photoCamera
                let format = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
                var modeFields: [String: Any] = ["mode": photoMode ? "photo" : "live",
                    "photo_camera_selection": photoCamera.rawValue, "requested_device_type": device.deviceType.rawValue,
                    "requested_device_id": device.uniqueID,
                    "requested_zoom": desiredZoom, "configured_zoom": device.videoZoomFactor,
                    "active_format_width": format.width, "active_format_height": format.height,
                    "highest_photo_quality": device.activeFormat.isHighestPhotoQualitySupported,
                    "supported_max_photo_dimensions": device.activeFormat.supportedMaxPhotoDimensions.map {
                        ["width": $0.width, "height": $0.height]
                    }]
                if photoMode {
                    modeFields["device_inventory_max_photo_dimensions"] = CameraAcquisition.maximumPhotoDimensions(device.formats)
                    modeFields["highest_quality_inventory_max_photo_dimensions"] = CameraAcquisition.maximumPhotoDimensions(device.formats.filter { $0.isHighestPhotoQualitySupported })
                    if let wide = mainCamera {
                        modeFields["physical_wide_inventory_max_photo_dimensions"] = CameraAcquisition.maximumPhotoDimensions(wide.formats)
                    }
                }
                recordCameraEvent("camera_mode_configured", fields: modeFields)
            }
            guard state.lock.withLock({ self.state.cameraWanted && self.state.generation == ticket }) else { return }
            if !session.isRunning { session.startRunning() }
            if session.isRunning && !didRecordVideoFormatInventory,
               let automaticCamera,
               let output = session.outputs.compactMap({ $0 as? AVCaptureVideoDataOutput }).first {
                let inventory = CameraAcquisition.videoFormatInventoryFields(
                    automaticDevice: automaticCamera,
                    mainDevice: mainCamera,
                    session: session,
                    output: output
                )
                didRecordVideoFormatInventory = true
                diagnostics.recordVideoFormatInventory(inventory)
                recordCameraEvent("video_format_inventory", fields: [
                    "snapshot_file": "camera-video-formats.json",
                    "inventory_device_count": inventory["inventory_device_count"] as? Int ?? 0,
                    "recorded_device_count": inventory["recorded_device_count"] as? Int ?? 0,
                    "candidate_format_count": inventory["candidate_format_count"] as? Int ?? 0,
                    "distinct_candidate_format_count": inventory["distinct_candidate_format_count"] as? Int ?? 0,
                    "recorded_format_count": inventory["recorded_format_count"] as? Int ?? 0,
                    "inventory_truncated": (inventory["inventory_devices_truncated"] as? Bool ?? false)
                        || (inventory["candidate_formats_truncated"] as? Bool ?? false)
                ])
            }
            continuousFocus(reason: "camera_started")
            state.lock.withLock { if self.state.generation == ticket { self.state.cameraConfiguring = false } }
            let ready = session.isRunning && !session.isInterrupted
            DispatchQueue.main.async {
                guard self.state.isCurrent(ticket), self.state.lock.withLock({ self.state.cameraWanted }) else { return }
                let photoAvailable = self.session.outputs.contains(self.photoOutput) && self.photoOutput.availablePhotoCodecTypes.contains(.jpeg)
                let minimumDistance = self.camera?.minimumFocusDistance ?? -1
                let guidance = photoCamera == .main
                    ? (minimumDistance > 0 ? "Keep at least \(String(format: "%g", Double(minimumDistance) / 10)) cm away for focus." : "Move back until the main camera is in focus.")
                    : ""
                let dimensions = self.photoOutput.maxPhotoDimensions
                self.output(.presentation(CameraPresentation(
                    status: ready ? self.state.scanningStatus : "Camera unavailable · Check scan details",
                    cameraAvailable: true,
                    processingImage: self.state.lock.withLock { self.state.automaticDecoding && self.state.liveRecoveryActive },
                    liveCamera: ready,
                    photoResolution: "Up to \(dimensions.width) × \(dimensions.height) · processed still",
                    photoAvailable: photoAvailable,
                    mainCameraAvailable: self.mainCamera.flatMap(CameraAcquisition.photoFormat) != nil && photoAvailable,
                    photoFocusGuidance: guidance), ticket: ticket))
            }
        }
    }

    func observeCamera(_ device: AVCaptureDevice) {
        if let camera {
            NotificationCenter.default.removeObserver(self, name: AVCaptureDevice.subjectAreaDidChangeNotification, object: camera)
        }
        adjustingFocusObservation = nil
        lensObservation = nil
        camera = device
        state.lock.withLock { state.lensRevision += 1 }
        NotificationCenter.default.addObserver(self, selector: #selector(subjectAreaChanged(_:)),
                                               name: AVCaptureDevice.subjectAreaDidChangeNotification, object: device)
        let deviceID = device.uniqueID
        adjustingFocusObservation = device.observe(\.isAdjustingFocus, options: [.new]) { [weak self] _, change in
            guard let adjusting = change.newValue else { return }
            self?.sessionQueue.async { [weak self] in
                guard let self, self.camera?.uniqueID == deviceID,
                      self.state.lock.withLock({ self.state.cameraWanted }) else { return }
                self.publishFocusStatus(adjusting || self.focusObservation != nil ? "Focusing…" : "Continuous focus")
                self.recordCameraEvent("focus_adjustment", fields: ["adjusting": adjusting])
            }
        }
        lensObservation = device.observe(\.activePrimaryConstituent, options: [.new]) { [weak self] _, _ in
            guard let self else { return }
            self.state.lock.withLock { self.state.lensRevision += 1 }
            self.recordCameraEvent("active_lens_changed")
        }
    }

    func capturePreset(photoMode: Bool) -> AVCaptureSession.Preset? {
        if photoMode { return session.canSetSessionPreset(.photo) ? .photo : nil }
        if session.canSetSessionPreset(.hd4K3840x2160) { return .hd4K3840x2160 }
        return session.canSetSessionPreset(.hd1920x1080) ? .hd1920x1080 : nil
    }

    func selectPhotoCamera(_ selection: PhotoCamera) -> Bool? {
        state.lock.withLock {
            guard requestedPhotoCamera != selection, !state.photoInFlight else { return nil }
            requestedPhotoCamera = selection
            state.captureRequested = false
            state.generation += 1
            state.cameraConfiguring = state.cameraWanted
            return state.cameraWanted
        }
    }

    func scanPhoto() {
        let ticket = state.lock.withLock {
            state.generation += 1
            state.captureRequested = false
            state.photoInFlight = true
            return state.generation
        }
        output(.presentation(CameraPresentation(status: "Capturing high-detail photo… Hold steady.",
            processingImage: true, liveCamera: false), ticket: ticket))
        sessionQueue.async { [weak self] in
            guard let self, self.state.isCurrent(ticket), self.session.isRunning else { return }
            let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg,
                AVVideoCompressionPropertiesKey: [AVVideoQualityKey: 1.0]])
            settings.maxPhotoDimensions = self.photoOutput.maxPhotoDimensions
            settings.photoQualityPrioritization = .quality
            settings.flashMode = .off
            let captureID = UUID()
            let dimensions = settings.maxPhotoDimensions
            let fields: [String: Any] = ["capture_id": captureID.uuidString.lowercased(),
                "capture_kind": "processed_still", "photo_settings_id": settings.uniqueID,
                "requested_width": dimensions.width, "requested_height": dimensions.height,
                "quality_priority": "quality", "source_codec": "JPEG", "jpeg_quality": 1.0,
                "photo_camera_selection": self.configuredPhotoCamera.rawValue,
                "requested_device_type": self.camera?.deviceType.rawValue ?? "unknown",
                "requested_device_id": self.camera?.uniqueID ?? "unknown",
                "requested_zoom": self.camera?.videoZoomFactor ?? 1,
                "requested_at": Date().timeIntervalSince1970]
            let delegate = StillPhotoCapture { [weak self] outcome in
                guard let self else { return }
                self.sessionQueue.async { [self] in
                    self.photoCaptures[settings.uniqueID] = nil
                    guard self.state.lock.withLock({
                        guard self.state.generation == ticket else { return false }
                        self.state.cameraWanted = false
                        return true
                    }) else { return }
                    self.stopSession()
                    DispatchQueue.main.async {
                        guard self.state.isCurrent(ticket) else { return }
                        self.output(.presentation(CameraPresentation(status: "Reading captured photo…"), ticket: ticket))
                    }
                    self.output(.photo(outcome, metadata: fields, captureID: captureID, ticket: ticket))
                }
            }
            self.photoCaptures[settings.uniqueID] = delegate
            self.recordCameraEvent("photo_requested", fields: fields)
            self.photoOutput.capturePhoto(with: settings, delegate: delegate)
        }
    }
}

final class StillPhotoCapture: NSObject, AVCapturePhotoCaptureDelegate {
    private var result: Result<(Data, [String: Any]), Error>?
    private let completion: (Result<(Data, [String: Any]), Error>) -> Void

    init(completion: @escaping (Result<(Data, [String: Any]), Error>) -> Void) { self.completion = completion }

    func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
        result = Result {
            if let error { throw error }
            guard let data = photo.fileDataRepresentation() else { throw CWFailure(message: "Camera returned no photo pixels.") }
            var fields: [String: Any] = ["resolved_width": photo.resolvedSettings.photoDimensions.width,
                "resolved_height": photo.resolvedSettings.photoDimensions.height,
                "frame_timestamp_seconds": CMTimeGetSeconds(photo.timestamp),
                "photo_delivered_at": Date().timeIntervalSince1970]
            if let deviceType = photo.sourceDeviceType { fields["source_device_type"] = deviceType.rawValue }
            if let exif = photo.metadata[kCGImagePropertyExifDictionary as String] as? [String: Any] {
                let keys = [kCGImagePropertyExifExposureTime, kCGImagePropertyExifFNumber,
                    kCGImagePropertyExifISOSpeedRatings, kCGImagePropertyExifFocalLength,
                    kCGImagePropertyExifFocalLenIn35mmFilm, kCGImagePropertyExifLensModel,
                    kCGImagePropertyExifExposureBiasValue, kCGImagePropertyExifWhiteBalance]
                var exposure: [String: Any] = [:]
                for key in keys { exposure[key as String] = exif[key as String] }
                fields["photo_exif"] = exposure
            }
            return (data, fields)
        }
    }

    func photoOutput(_ output: AVCapturePhotoOutput, didFinishCaptureFor resolvedSettings: AVCaptureResolvedPhotoSettings, error: Error?) {
        if let error { completion(.failure(error)) }
        else { completion(result ?? .failure(CWFailure(message: "Camera did not deliver a processed photo."))) }
    }
}

extension CameraAcquisition {
    func recordCameraEvent(_ event: String, fields: [String: Any] = [:]) {
        let timestamp = Date().timeIntervalSince1970
        sessionQueue.async { [weak self] in
            guard let self else { return }
            var entry = fields
            entry["event"] = event
            entry["timestamp"] = event == "decode_started" ? (fields["decode_started_at_unix_seconds"] ?? timestamp) : timestamp
            entry["session_running"] = self.session.isRunning
            entry["session_interrupted"] = self.session.isInterrupted
            entry["thermal_state"] = ProcessInfo.processInfo.thermalState.rawValue
            let routine = event == "decode_started" || event == "transfer_decode_failed" ||
                event == "transfer_chunk" || event == "transfer_duplicate" || event == "live_frame_pending" || event == "decode_failure"
            let now = CACurrentMediaTime()
            if let device = self.camera, !routine || now - self.cameraDiagnosticSnapshotAt >= 0.25 || self.cameraDiagnosticSnapshot.isEmpty {
                let primary = device.activePrimaryConstituent ?? device
                let deviceDimensions = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
                let primaryDimensions = CMVideoFormatDescriptionGetDimensions(primary.activeFormat.formatDescription)
                let gains = device.deviceWhiteBalanceGains
                self.cameraDiagnosticSnapshot = [
                    "photo_camera_selection": self.state.lock.withLock { self.requestedPhotoCamera.rawValue },
                    "configured_photo_camera": self.configuredPhotoCamera.rawValue,
                    "device_id": device.uniqueID,
                    "active_lens_id": primary.uniqueID,
                    "sampled_at": Date().timeIntervalSince1970,
                    "device_type": device.deviceType.rawValue,
                    "capture_device_is_virtual": device.activePrimaryConstituent != nil,
                    "capture_device_format_width": Int(deviceDimensions.width),
                    "capture_device_format_height": Int(deviceDimensions.height),
                    "capture_device_is_video_binned": device.activeFormat.isVideoBinned,
                    "capture_device_video_zoom_factor": device.videoZoomFactor,
                    "capture_device_video_zoom_factor_upscale_threshold": device.activeFormat.videoZoomFactorUpscaleThreshold,
                    "active_lens": primary.deviceType.rawValue,
                    "active_primary_device_id": primary.uniqueID,
                    "active_primary_format_width": Int(primaryDimensions.width),
                    "active_primary_format_height": Int(primaryDimensions.height),
                    "active_primary_is_video_binned": primary.activeFormat.isVideoBinned,
                    "active_primary_video_zoom_factor": primary.videoZoomFactor,
                    "active_primary_video_zoom_factor_upscale_threshold": primary.activeFormat.videoZoomFactorUpscaleThreshold,
                    "focus_mode": device.focusMode.rawValue,
                    "lens_switching_behavior": device.primaryConstituentDeviceSwitchingBehavior.rawValue,
                    "adjusting_focus": device.isAdjustingFocus,
                    "lens_position": primary.lensPosition,
                    "capture_device_lens_position": device.lensPosition,
                    "active_primary_lens_position": primary.lensPosition,
                    "minimum_focus_distance_mm": primary.minimumFocusDistance,
                    "focus_x": device.focusPointOfInterest.x, "focus_y": device.focusPointOfInterest.y,
                    "zoom": device.videoZoomFactor,
                    "iso": device.iso, "exposure_seconds": CMTimeGetSeconds(device.exposureDuration),
                    "active_max_exposure_duration_seconds": CMTimeGetSeconds(device.activeMaxExposureDuration),
                    "video_min_frame_duration_seconds": CMTimeGetSeconds(device.activeVideoMinFrameDuration),
                    "video_max_frame_duration_seconds": CMTimeGetSeconds(device.activeVideoMaxFrameDuration),
                    "system_pressure_level": device.systemPressureState.level.rawValue,
                    "active_primary_max_exposure_duration_seconds": CMTimeGetSeconds(primary.activeMaxExposureDuration),
                    "white_balance_red": gains.redGain, "white_balance_green": gains.greenGain,
                    "white_balance_blue": gains.blueGain,
                    "active_color_space": device.activeColorSpace.rawValue,
                    "active_color_space_name": String(describing: device.activeColorSpace),
                    "supported_color_spaces": device.activeFormat.supportedColorSpaces.map { $0.rawValue },
                    "video_hdr_enabled": device.isVideoHDREnabled,
                    "automatically_adjusts_video_hdr_enabled": device.automaticallyAdjustsVideoHDREnabled,
                    "automatically_configures_capture_device_for_wide_color": self.session.automaticallyConfiguresCaptureDeviceForWideColor,
                    "preset": self.session.sessionPreset.rawValue,
                    "photo_output_available": self.session.outputs.contains(self.photoOutput) && self.photoOutput.availablePhotoCodecTypes.contains(.jpeg),
                    "photo_output_max_width": self.photoOutput.maxPhotoDimensions.width,
                    "photo_output_max_height": self.photoOutput.maxPhotoDimensions.height
                ] as [String: Any]
                self.cameraDiagnosticSnapshotAt = now
            }
            if !self.cameraDiagnosticSnapshot.isEmpty { entry["camera"] = self.cameraDiagnosticSnapshot }
            self.diagnostics.record(entry)
        }
    }

}
