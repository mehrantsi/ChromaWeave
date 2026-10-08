import Foundation
import UIKit
import SwiftUI
import UniformTypeIdentifiers
import ImageIO

struct BinaryDocument: FileDocument {
    static var readableContentTypes: [UTType] { [.data] }
    static var writableContentTypes: [UTType] { [.data, .gif] }
    let data: Data
    init(data: Data) { self.data = data }
    init(configuration: ReadConfiguration) throws { data = configuration.file.regularFileContents ?? Data() }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: data) }
}

enum CodeSample: String, CaseIterable, Identifiable {
    case text, png, svg, binary
    var id: String { rawValue }
    var title: String {
        switch self { case .text: "Text note"; case .png: "Small image"; case .svg: "SVG artwork"; case .binary: "2 KiB binary" }
    }
    var data: Data {
        switch self {
        case .text:
            return Data("""
            A note transferred with CW Code.

            This text is stored inside the code image.
            The decoder recovers its original UTF-8 bytes.

            ASCII is supported. So is UTF-8: København · 東京 · ☀
            """.utf8)
        case .svg:
            return Data("""
            <svg xmlns="http://www.w3.org/2000/svg" width="320" height="240" viewBox="0 0 320 240"><rect width="320" height="240" rx="12" fill="#f4f1e8"/><circle cx="160" cy="114" r="78" fill="#10645b"/><path d="M58 169L127 64l64 104 44-70 38 71Z" fill="#f5bd58"/><circle cx="211" cy="62" r="23" fill="#e07050"/><path d="M32 201h256" stroke="#19312e" stroke-width="2"/></svg>
            """.utf8)
        case .png:
            let format = UIGraphicsImageRendererFormat()
            format.scale = 1
            format.opaque = true
            return UIGraphicsImageRenderer(size: CGSize(width: 96, height: 96), format: format).pngData { context in
                UIColor(red: 0.96, green: 0.94, blue: 0.9, alpha: 1).setFill()
                context.fill(CGRect(x: 0, y: 0, width: 96, height: 96))
                UIColor(red: 0.06, green: 0.39, blue: 0.36, alpha: 1).setFill()
                context.cgContext.fillEllipse(in: CGRect(x: 12, y: 12, width: 72, height: 72))
                UIColor(red: 0.96, green: 0.74, blue: 0.34, alpha: 1).setFill()
                let triangle = UIBezierPath()
                triangle.move(to: CGPoint(x: 15, y: 72)); triangle.addLine(to: CGPoint(x: 48, y: 23))
                triangle.addLine(to: CGPoint(x: 81, y: 72)); triangle.close(); triangle.fill()
                UIColor(red: 0.88, green: 0.44, blue: 0.31, alpha: 1).setFill()
                context.cgContext.fillEllipse(in: CGRect(x: 62, y: 10, width: 20, height: 20))
            }
        case .binary: return Codec.sample
        }
    }
}

struct PayloadContent {
    let data: Data
    var isGIF: Bool { data.starts(with: Data("GIF87a".utf8)) || data.starts(with: Data("GIF89a".utf8)) }
    var gifFrameCount: Int {
        guard isGIF, let source = CGImageSourceCreateWithData(data as CFData, nil) else { return 0 }
        return CGImageSourceGetCount(source)
    }
    var text: String? {
        if isGIF { return nil }
        if data.starts(with: [0xff, 0xfe]) || data.starts(with: [0xfe, 0xff]) { return String(data: data, encoding: .utf16) }
        return String(data: data, encoding: .utf8)
    }
    var isSVG: Bool {
        guard let text else { return false }
        let prefix = text.trimmingCharacters(in: .whitespacesAndNewlines).prefix(2048).lowercased()
        return prefix.hasPrefix("<svg") || ((prefix.hasPrefix("<?xml") || prefix.hasPrefix("<!--")) && prefix.contains("<svg"))
    }
    var image: UIImage? { isSVG || isGIF ? nil : UIImage(data: data) }
    var inertImageMIMEType: String? {
        if isGIF { return "image/gif" }
        if isSVG { return "image/svg+xml" }
        return nil
    }
    var exportContentType: UTType { isGIF ? .gif : .data }
    var label: String {
        if isGIF { return gifFrameCount > 1 ? "Animated GIF" : "GIF image" }
        if isSVG { return "SVG image" }
        if image != nil { return "Raster image" }
        if text != nil { return data.allSatisfy { $0 < 128 } ? "ASCII text" : "Unicode text" }
        return "Binary data"
    }
    var fileExtension: String {
        if isGIF { return "gif" }
        if isSVG { return "svg" }
        if data.starts(with: [137,80,78,71,13,10,26,10]) { return "png" }
        if data.starts(with: [255,216,255]) { return "jpg" }
        return text == nil ? "bin" : "txt"
    }
    var hex: String { hexadecimal(limit: 8192) }
    func hexadecimal(limit: Int? = nil) -> String {
        let bytes = Array(data.prefix(limit ?? data.count))
        return stride(from: 0, to: bytes.count, by: 8).map { offset in
            let row = bytes[offset..<min(offset + 8, bytes.count)]
            let hex = row.map { String(format: "%02x", $0) }.joined(separator: " ").padding(toLength: 23, withPad: " ", startingAt: 0)
            let ascii = row.map { byte in byte >= 32 && byte <= 126 ? String(UnicodeScalar(byte)) : "·" }.joined()
            return String(format: "%06X  ", offset) + hex + "  |" + ascii + "|"
        }.joined(separator: "\n")
    }
}
