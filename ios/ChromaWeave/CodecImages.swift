import Foundation
import UIKit
import ImageIO

extension Codec {
    static func decode(image: UIImage) throws -> DecodedPayload {
        let size = CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
        guard size.width > 0, size.height > 0, size.width * size.height <= 40_000_000 else {
            throw CWFailure(message: "Choose an image with at most 40 megapixels.")
        }
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let normalized = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
        guard let cgImage = normalized.cgImage else { throw CWFailure(message: "Cannot read image pixels.") }
        let width = cgImage.width
        let height = cgImage.height
        let stride = width * 4
        var pixels = [UInt8](repeating: 0, count: stride * height)
        return try pixels.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: stride, space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue) else {
                throw CWFailure(message: "Cannot prepare image pixels.")
            }
            context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
            return try decode(pixels: buffer.bindMemory(to: UInt8.self).baseAddress!, length: buffer.count,
                              width: width, height: height, stride: stride)
        }
    }

    static func frame(pixels: UnsafeRawPointer, width: Int, height: Int, stride: Int, colorSpace: CGColorSpace = CGColorSpaceCreateDeviceRGB()) throws -> CapturedFrame {
        let data = Data(bytes: pixels, count: stride * height)
        guard let provider = CGDataProvider(data: data as CFData),
              let image = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                                  bytesPerRow: stride, space: colorSpace,
                                  bitmapInfo: CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.noneSkipFirst.rawValue),
                                  provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent) else {
            throw CWFailure(message: "Cannot prepare the captured frame.")
        }
        return CapturedFrame(image: UIImage(cgImage: image), width: width, height: height)
    }

    static func withPhotoPixels<T>(_ data: Data,
        body: (UnsafeMutableRawBufferPointer, Int, Int, Int, Int, Int) throws -> T) throws -> (T, [String: Any]) {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [String: Any],
              let sourceWidth = properties[kCGImagePropertyPixelWidth as String] as? Int,
              let sourceHeight = properties[kCGImagePropertyPixelHeight as String] as? Int,
              sourceWidth > 0, sourceHeight > 0 else { throw CWFailure(message: "Cannot read captured photo dimensions.") }
        let maximumSide = photoMaximumSide(width: sourceWidth, height: sourceHeight)
        let options: [CFString: Any] = [kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true, kCGImageSourceThumbnailMaxPixelSize: maximumSide,
            kCGImageSourceShouldCacheImmediately: true]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
              image.width * image.height <= 40_000_000 else { throw CWFailure(message: "Cannot prepare this photo within the 40 megapixel limit.") }
        let width = image.width, height = image.height, stride = width * 4
        var pixels = Data(count: stride * height)
        let started = CACurrentMediaTime()
        let value = try pixels.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                bytesPerRow: stride, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.noneSkipFirst.rawValue) else {
                throw CWFailure(message: "Cannot prepare captured photo pixels.")
            }
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return try body(buffer, width, height, stride, sourceWidth, sourceHeight)
        }
        let fields: [String: Any] = ["source_width": sourceWidth, "source_height": sourceHeight,
            "source_orientation": properties[kCGImagePropertyOrientation as String] ?? 1,
            "width": width, "height": height, "stride": stride, "pixel_format": "BGRA8_sRGB",
            "resize_applied": width * height < sourceWidth * sourceHeight,
            "resize_scale": Double(max(width, height)) / Double(max(sourceWidth, sourceHeight)),
            "decode_attempted": true, "decode_ms": (CACurrentMediaTime() - started) * 1000]
        return (value, fields)
    }

    static func scanPhoto(_ data: Data, captureID: UUID,
                          recovery: ((UnsafePointer<UInt8>, Int, Int, Int, Int) throws -> PhotoRecoveryResult)? = nil) throws -> (CapturedFrame, [String: Any]) {
        let (frame, preparation) = try withPhotoPixels(data) { buffer, width, height, stride, sourceWidth, sourceHeight in
            let pixels = buffer.bindMemory(to: UInt8.self).baseAddress!
            let recoveryOutcome = Result {
                if let recovery {
                    return try recovery(pixels, buffer.count, width, height, stride)
                }
                return try recoverSinglePhoto(pixels: pixels, length: buffer.count, width: width, height: height,
                                              stride: stride, captureID: captureID)
            }
            var frame = try self.frame(pixels: buffer.baseAddress!, width: width, height: height, stride: stride, colorSpace: CGColorSpace(name: CGColorSpace.sRGB)!)
            frame.id = captureID
            frame.isPhoto = true
            frame.captureDetails = "Captured \(sourceWidth) × \(sourceHeight). " +
                (width * height < sourceWidth * sourceHeight ? "Reduced to the decoder’s 40 MP limit; orientation normalized." : "Full captured resolution; orientation normalized.")
            switch recoveryOutcome {
            case .success(let recovered):
                frame.recoveryStatus = recovered.status
                frame.recoveryMessage = recovered.message
                frame.recoveryFrames = recovered.frames
                frame.recoveryMaxFrames = recovered.maxFrames
                frame.recoveryCertifiedBlocks = recovered.certifiedBlocks
                frame.recoveryTotalBlocks = recovered.totalBlocks
                frame.recoverySessionID = recovered.sessionID
                frame.recoveryCaptureIDs = recovered.captureIDs
                if recovered.status == "decoded" { frame.decodedPayload = recovered.payload }
                if recovered.status == "error" { frame.decodeError = recovered.message ?? "No CW payload recovered." }
            case .failure(let recoveryError):
                frame.recoveryStatus = "error"
                frame.recoveryMessage = recoveryError.localizedDescription
                frame.decodeError = recoveryError.localizedDescription
            }
            return frame
        }
        var fields = preparation
        fields["decoded"] = frame.decodedPayload != nil
        if let recoveryStatus = frame.recoveryStatus {
            fields["recovery_status"] = recoveryStatus
            fields["recovery_session_id"] = frame.recoverySessionID
            fields["recovery_capture_ids"] = frame.recoveryCaptureIDs
            fields["recovery_frames"] = frame.recoveryFrames
            fields["recovery_max_frames"] = frame.recoveryMaxFrames
            fields["recovery_certified_blocks"] = frame.recoveryCertifiedBlocks
            fields["recovery_total_blocks"] = frame.recoveryTotalBlocks
            fields["recovery_message"] = frame.recoveryMessage
        }
        if let decoded = frame.decodedPayload {
            fields["profile"] = decoded.profile
            fields["payload_length"] = decoded.data.count
            fields["corrected"] = decoded.corrected
            fields["erased"] = decoded.erased
        }
        if let error = frame.decodeError { fields["error"] = error }
        return (frame, fields)
    }

    static func photoMaximumSide(width: Int, height: Int) -> Int {
        let scale = min(1, sqrt(40_000_000 / (Double(width) * Double(height))))
        return max(1, Int((Double(max(width, height)) * scale).rounded(.down)))
    }
}
