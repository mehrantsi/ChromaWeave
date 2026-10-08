import CWCodec
import Foundation
import UIKit
import UniformTypeIdentifiers
import SwiftUI
import ImageIO
import CoreMedia
import CoreVideo

enum Codec {
    static let defaultProfile = 3
    static let sample = Data((0..<2048).map { UInt8(truncatingIfNeeded: $0 * 131 + 47) })
    static let maximumCompressedInputBytes = 16 * 1024 * 1024

    static func maximumPayloadBytes(profile: Int) -> Int? {
        switch profile {
        case 3: 198_420
        case 4: 258_324
        default: nil
        }
    }

    static func maximumInputBytes(profile: Int, losslessCompression: Bool) -> Int? {
        guard let maximumPayloadBytes = maximumPayloadBytes(profile: profile) else { return nil }
        return losslessCompression ? maximumCompressedInputBytes : maximumPayloadBytes
    }

    static func repairTransfer(dataShards: Int, shards: [Data?]) throws -> [(index: Int, data: Data)] {
        guard (1...60).contains(dataShards), shards.count == dataShards + 4,
              let size = shards.compactMap({ $0?.count }).first, [10_240, 20_480].contains(size) else {
            throw CWFailure(message: "Invalid transfer repair group.")
        }
        var indices = [UInt8]()
        var packed = Data()
        packed.reserveCapacity(shards.count * size)
        for (index, shard) in shards.enumerated() {
            guard let shard else { continue }
            guard shard.count == size else { throw CWFailure(message: "Inconsistent transfer shard size.") }
            indices.append(UInt8(index))
            packed.append(shard)
        }
        let result = try indices.withUnsafeBufferPointer { indices in
            try packed.withUnsafeBytes { bytes in
                try response(cw_transfer_repair(UInt8(dataShards), UInt32(size), indices.baseAddress,
                    indices.count, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count), as: TransferRepairResponse.self)
            }
        }
        guard result.ok, let recovered = result.recovered else {
            throw CWFailure(message: result.message ?? "Transfer repair failed.")
        }
        var seen = Set<Int>()
        return try recovered.map { item in
            guard (0..<dataShards).contains(item.index),
                  shards[item.index] == nil, seen.insert(item.index).inserted,
                  item.dataHex.utf8.count == size * 2 else {
                throw CWFailure(message: "Invalid repaired transfer shard.")
            }
            let characters = Array(item.dataHex.utf8)
            func nibble(_ byte: UInt8) throws -> UInt8 {
                switch byte {
                case 48...57: return byte - 48
                case 97...102: return byte - 87
                default: throw CWFailure(message: "Invalid repaired transfer bytes.")
                }
            }
            var bytes = Data(count: size)
            try bytes.withUnsafeMutableBytes { (output: UnsafeMutableRawBufferPointer) in
                for i in 0..<size {
                    output[i] = try nibble(characters[i * 2]) << 4 | nibble(characters[i * 2 + 1])
                }
            }
            return (item.index, bytes)
        }
    }

    static func decode(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int) throws -> DecodedPayload {
        let result = try response(cw_decode_bgra(pixels, length, UInt32(width), UInt32(height), stride), as: CodecResponse.self)
        return try result.decodedPayload()
    }

    static func decodeTransfer(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int,
                               session: UnsafeMutableRawPointer) throws -> DecodedPayload {
        let result = try response(cw_transfer_scanner_decode_bgra(session, pixels, length, UInt32(width), UInt32(height), stride), as: CodecResponse.self)
        return try result.decodedPayload()
    }

    static func decodeRecovery(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int,
                               session: UnsafeMutableRawPointer, sessionID: UUID,
                               captureIDs: [UUID]) throws -> PhotoRecoveryResult {
        let result = try response(cw_recovery_decode_bgra(session, pixels, length, UInt32(width), UInt32(height), stride),
            as: CodecResponse.self)
        return try result.recovery(sessionID: sessionID, captureIDs: captureIDs)
    }

    static func recoverSinglePhoto(pixels: UnsafePointer<UInt8>, length: Int, width: Int, height: Int, stride: Int,
                                   captureID: UUID) throws -> PhotoRecoveryResult {
        try RecoverySession().decode(pixels: pixels, length: length, width: width, height: height,
            stride: stride, captureID: captureID)
    }

    static func encode(_ payload: Data, profile: Int = defaultProfile, cellPixels: Int = 8,
                       losslessCompression: Bool = false) throws -> EncodedCode {
        guard profile == 3 || profile == 4 else { throw CWFailure(message: "Unknown CW profile.") }
        let result = try payload.withUnsafeBytes { bytes -> CodecResponse in
            let pointer = bytes.bindMemory(to: UInt8.self).baseAddress
            if losslessCompression {
                return try response(cw_encode_png_with_options(
                    pointer, bytes.count, UInt32(profile), UInt32(cellPixels), 1), as: CodecResponse.self)
            }
            return try response(cw_encode_png(pointer, bytes.count, UInt32(profile), UInt32(cellPixels)), as: CodecResponse.self)
        }
        guard result.status == .encoded, let bytes = result.png,
              let side = result.sidePixels, side > 0,
              let storedBytes = result.encodedPayloadBytes, storedBytes >= 0, let compression = result.compression else {
            throw CWFailure(message: result.message ?? "Cannot encode this payload.")
        }
        let png = Data(bytes)
        guard let image = UIImage(data: png) else { throw CWFailure(message: "Cannot display the encoded image.") }
        return EncodedCode(png: png, image: image, side: side, bytes: payload.count,
                           storedBytes: storedBytes, compression: compression)
    }
}
