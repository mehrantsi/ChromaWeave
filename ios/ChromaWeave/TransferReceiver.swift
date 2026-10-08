import CryptoKit
import Foundation

struct TransferReceiver {
    typealias Repair = (_ dataShards: Int, _ shards: [Data?]) throws -> [(index: Int, data: Data)]

    struct Update {
        let acceptedUnique: Bool
        let duplicate: Bool
        let completedData: Data?
        let transferIDHex: String
        let chunkIndex: Int
        let originalLength: Int
        let chunkSize: Int
        let uniqueValidatedBytes: Int
        let receivedChunks: Int
        let totalChunks: Int
        let missingChunks: Int
        let missingIndices: [Int]?
        let duplicateCount: Int
        let expectedSHA256Hex: String
        let isComplete: Bool
        let envelope: String
        let sourceChunkIndex: Int?
        let repairSymbols: Int
        let repairedChunks: Int
        let acceptedRepairSymbol: Bool

        var receivedBytes: Int { uniqueValidatedBytes }
    }

    enum Failure: Error, Equatable, LocalizedError {
        case truncatedHeader
        case invalidMagic
        case invalidPacketLength
        case unsupportedChunkSize
        case invalidOriginalLength
        case invalidChunkCount
        case invalidChunkIndex
        case invalidChunkLength
        case truncatedChunk
        case foreignTransfer
        case metadataMismatch
        case conflictingDuplicate
        case digestMismatch
        case invalidRepairHeader
        case invalidSourcePadding
        case unsupportedRepair
        case invalidRepairResult

        var errorDescription: String? {
            switch self {
            case .truncatedHeader: "The transfer chunk header is incomplete."
            case .invalidMagic: "The packet is not a CW transfer chunk."
            case .invalidPacketLength: "The transfer chunk is too large."
            case .unsupportedChunkSize: "The transfer uses an unsupported chunk size."
            case .invalidOriginalLength: "The transfer length is outside the supported range."
            case .invalidChunkCount: "The transfer chunk count is invalid."
            case .invalidChunkIndex: "The transfer chunk index is invalid."
            case .invalidChunkLength: "The transfer chunk length is invalid."
            case .truncatedChunk: "The transfer chunk data is incomplete."
            case .foreignTransfer: "A different transfer is already active. Reset before starting another transfer."
            case .metadataMismatch: "The transfer metadata changed between chunks."
            case .conflictingDuplicate: "A repeated chunk has different data."
            case .digestMismatch: "The completed transfer does not match its SHA-256 digest. Reset to try again."
            case .invalidRepairHeader: "The CWT repair group is invalid."
            case .invalidSourcePadding: "The CWT source padding is nonzero."
            case .unsupportedRepair: "CWT repair is unavailable."
            case .invalidRepairResult: "The CWT repair result is invalid."
            }
        }
    }

    private struct Metadata: Equatable {
        let transferID: Data
        let index: Int
        let count: Int
        let chunkSize: Int
        let originalLength: Int
        let digest: Data
        let groupData: Int

        var sourceIndex: Int? {
            let start = index / (groupData + 4) * groupData
            let local = index % (groupData + 4)
            return local < min(groupData, count - start) ? start + local : nil
        }

        var transferMetadata: TransferMetadata {
            TransferMetadata(transferID: transferID, count: count, chunkSize: chunkSize,
                             originalLength: originalLength, digest: digest, groupData: groupData)
        }
    }

    private struct TransferMetadata: Equatable {
        let transferID: Data
        let count: Int
        let chunkSize: Int
        let originalLength: Int
        let digest: Data
        let groupData: Int

        func sourceLength(_ index: Int) -> Int {
            min(chunkSize, originalLength - index * chunkSize)
        }
    }

    private struct Chunk {
        let metadata: Metadata
        let bytes: Data
    }

    private final class ActiveTransfer {
        let metadata: TransferMetadata
        var chunks = [Int: Data]()
        var parity = [Int: Data]()
        var repairSymbols = 0
        var repairedChunks = 0
        var receivedChunks = 0
        var uniqueValidatedBytes = 0
        var duplicateCount = 0
        var finalData: Data?
        var digestMismatch = false

        init(metadata: TransferMetadata) {
            self.metadata = metadata
        }
    }

    static let maximumOriginalLength = 32 * 1024 * 1024
    private static let headerLength = 64
    private static let supportedChunkSizes: Set<Int> = [10_240, 20_480]
    private var active: ActiveTransfer?
    private let repair: Repair?

    init(repair: Repair? = nil) {
        self.repair = repair
    }

    var transferIDHex: String? {
        active.map { Self.hex($0.metadata.transferID) }
    }

    mutating func accept(_ packet: Data) throws -> Update {
        let chunk = try Self.parse(packet)
        let transfer: ActiveTransfer
        if let active {
            guard active.metadata.transferID == chunk.metadata.transferID else { throw Failure.foreignTransfer }
            guard active.metadata == chunk.metadata.transferMetadata else { throw Failure.metadataMismatch }
            guard !active.digestMismatch else { throw Failure.digestMismatch }
            transfer = active
        } else {
            transfer = ActiveTransfer(metadata: chunk.metadata.transferMetadata)
        }

        let metadata = chunk.metadata
        let groupData = metadata.groupData
        let group = metadata.index / (groupData + 4)
        let start = group * groupData
        let count = min(groupData, metadata.count - start)
        let local = metadata.index % (groupData + 4)
        let sourceIndex = metadata.sourceIndex
        let existing: Data?
        if let sourceIndex, let finalData = transfer.finalData {
            existing = Self.paddedSource(finalData, metadata: transfer.metadata, index: sourceIndex)
        } else if let sourceIndex {
            existing = transfer.chunks[sourceIndex]
        } else {
            existing = transfer.parity[metadata.index]
        }
        if let existing {
            guard existing == chunk.bytes else { throw Failure.conflictingDuplicate }
            transfer.duplicateCount += 1
            active = transfer
            return Self.update(transfer, index: metadata.index, acceptedUnique: false, duplicate: true, completedData: nil)
        }

        var shards: [Data?] = (0..<count).map { offset in
            if let finalData = transfer.finalData {
                return Self.paddedSource(finalData, metadata: transfer.metadata, index: start + offset)
            }
            return transfer.chunks[start + offset]
        }
        shards += (0..<4).map { transfer.parity[group * (groupData + 4) + count + $0] }
        shards[local] = chunk.bytes
        let missing = (0..<count).filter { shards[$0] == nil }
        var restored = [(index: Int, data: Data)]()
        if shards.compactMap({ $0 }).count >= count, shards[count...].contains(where: { $0 != nil }) {
            guard let repair else { throw Failure.unsupportedRepair }
            restored = try repair(count, shards)
            guard restored.count == missing.count, Set(restored.map(\.index)) == Set(missing) else {
                throw Failure.invalidRepairResult
            }
            for (index, data) in restored {
                guard data.count == metadata.chunkSize else { throw Failure.invalidRepairResult }
                guard data.dropFirst(transfer.metadata.sourceLength(start + index)).allSatisfy({ $0 == 0 }) else {
                    throw Failure.invalidSourcePadding
                }
            }
        }

        let previousBytes = transfer.uniqueValidatedBytes
        if let sourceIndex {
            transfer.chunks[sourceIndex] = chunk.bytes
            transfer.receivedChunks += 1
            transfer.uniqueValidatedBytes += transfer.metadata.sourceLength(sourceIndex)
        } else {
            transfer.parity[metadata.index] = chunk.bytes
            transfer.repairSymbols += 1
        }
        for (index, data) in restored {
            transfer.chunks[start + index] = data
            transfer.receivedChunks += 1
            transfer.repairedChunks += 1
            transfer.uniqueValidatedBytes += transfer.metadata.sourceLength(start + index)
        }
        active = transfer
        let completed = try Self.finish(transfer)
        return Self.update(transfer, index: metadata.index, acceptedUnique: transfer.uniqueValidatedBytes > previousBytes,
                           duplicate: false, completedData: completed, acceptedRepairSymbol: sourceIndex == nil)
    }

    private static func paddedSource(_ data: Data, metadata: TransferMetadata, index: Int) -> Data {
        let start = index * metadata.chunkSize
        var source = data.subdata(in: start..<(start + metadata.sourceLength(index)))
        source.append(Data(repeating: 0, count: metadata.chunkSize - source.count))
        return source
    }

    private static func finish(_ transfer: ActiveTransfer) throws -> Data? {
        guard transfer.finalData == nil, transfer.receivedChunks == transfer.metadata.count else { return nil }
        var assembled = Data()
        assembled.reserveCapacity(transfer.metadata.originalLength)
        for index in 0..<transfer.metadata.count {
            guard let bytes = transfer.chunks[index] else { throw Failure.invalidChunkCount }
            assembled.append(bytes.prefix(transfer.metadata.sourceLength(index)))
        }
        guard assembled.count == transfer.metadata.originalLength,
              Data(SHA256.hash(data: assembled)) == transfer.metadata.digest else {
            transfer.chunks.removeAll(keepingCapacity: false)
            transfer.parity.removeAll(keepingCapacity: false)
            transfer.digestMismatch = true
            throw Failure.digestMismatch
        }
        transfer.finalData = assembled
        transfer.chunks.removeAll(keepingCapacity: false)
        return assembled
    }

    mutating func reset() {
        active = nil
    }


    private static func parse(_ packet: Data) throws -> Chunk {
        guard packet.count >= headerLength else { throw Failure.truncatedHeader }
        guard packet.count <= headerLength + 20_480 else { throw Failure.invalidPacketLength }
        guard packet.prefix(4).elementsEqual(Data("CWT\0".utf8)) else { throw Failure.invalidMagic }
        let index = Int(readUInt16(packet, at: 20))
        let count = Int(readUInt16(packet, at: 22))
        let chunkSize = Int(readUInt16(packet, at: 24))
        let originalLength = Int(readUInt32(packet, at: 26))
        let groupData = Int(packet[62])
        guard supportedChunkSizes.contains(chunkSize) else { throw Failure.unsupportedChunkSize }
        guard originalLength > 0, originalLength <= maximumOriginalLength else { throw Failure.invalidOriginalLength }
        guard groupData == 20 || groupData == 60, packet[63] == 4 else { throw Failure.invalidRepairHeader }
        guard count == (originalLength + chunkSize - 1) / chunkSize else { throw Failure.invalidChunkCount }
        let groupCount = (count + groupData - 1) / groupData
        let symbols = count + groupCount * 4
        guard symbols <= Int(UInt16.max) + 1, index < symbols else { throw Failure.invalidChunkIndex }
        let groupStart = index / (groupData + 4) * groupData
        let groupSources = min(groupData, count - groupStart)
        guard groupStart < count, index % (groupData + 4) < groupSources + 4 else { throw Failure.invalidChunkIndex }
        guard packet.count == headerLength + chunkSize else {
            throw packet.count < headerLength + chunkSize ? Failure.truncatedChunk : Failure.invalidChunkLength
        }
        let metadata = Metadata(transferID: packet.subdata(in: 4..<20), index: index, count: count,
                                chunkSize: chunkSize, originalLength: originalLength,
                                digest: packet.subdata(in: 30..<62), groupData: groupData)
        let bytes = packet.subdata(in: headerLength..<packet.count)
        if let sourceIndex = metadata.sourceIndex {
            guard bytes.dropFirst(metadata.transferMetadata.sourceLength(sourceIndex)).allSatisfy({ $0 == 0 }) else {
                throw Failure.invalidSourcePadding
            }
        }
        return Chunk(metadata: metadata, bytes: bytes)
    }

    private static func update(_ transfer: ActiveTransfer, index: Int, acceptedUnique: Bool, duplicate: Bool,
                               completedData: Data?, acceptedRepairSymbol: Bool = false) -> Update {
        let groupData = transfer.metadata.groupData
        let start = index / (groupData + 4) * groupData
        let local = index % (groupData + 4)
        let sourceIndex = local < min(groupData, transfer.metadata.count - start) ? start + local : nil
        return Update(acceptedUnique: acceptedUnique, duplicate: duplicate, completedData: completedData,
               transferIDHex: hex(transfer.metadata.transferID), chunkIndex: index, originalLength: transfer.metadata.originalLength,
               chunkSize: transfer.metadata.chunkSize, uniqueValidatedBytes: transfer.uniqueValidatedBytes,
               receivedChunks: transfer.receivedChunks, totalChunks: transfer.metadata.count,
               missingChunks: transfer.metadata.count - transfer.receivedChunks,
               missingIndices: transfer.finalData != nil ? [] : transfer.metadata.count - transfer.receivedChunks <= 8
                    ? (0..<transfer.metadata.count).filter { transfer.chunks[$0] == nil } : nil,
               duplicateCount: transfer.duplicateCount, expectedSHA256Hex: hex(transfer.metadata.digest),
               isComplete: transfer.finalData != nil,
               envelope: "CWT",
               sourceChunkIndex: sourceIndex,
               repairSymbols: transfer.repairSymbols, repairedChunks: transfer.repairedChunks,
               acceptedRepairSymbol: acceptedRepairSymbol)
    }

    private static func readUInt16(_ data: Data, at offset: Int) -> UInt16 {
        UInt16(data[offset]) | (UInt16(data[offset + 1]) << 8)
    }

    private static func readUInt32(_ data: Data, at offset: Int) -> UInt32 {
        UInt32(data[offset]) | (UInt32(data[offset + 1]) << 8) |
        (UInt32(data[offset + 2]) << 16) | (UInt32(data[offset + 3]) << 24)
    }

    private static func hex(_ data: Data) -> String {
        data.map { String(format: "%02x", $0) }.joined()
    }


}

struct TransferRateMeter {
    private struct Bucket {
        var index: Int?
        var bytes: Int
    }

    private static let bucketDuration = 0.1
    private static let bucketCount = 20
    private static let liveWindow = 2.0
    private static let warmupDuration = 0.25

    let startedAt: Double
    private(set) var totalBytes = 0
    private var buckets: [Bucket]
    private var latestAcceptedAt: Double?

    init(startedAt: Double) {
        self.startedAt = startedAt
        self.buckets = Array(repeating: Bucket(index: nil, bytes: 0), count: Self.bucketCount)
    }

    mutating func record(totalBytes: Int, at time: Double) {
        guard totalBytes > self.totalBytes, startedAt.isFinite, time.isFinite,
              time >= startedAt, time >= (latestAcceptedAt ?? startedAt) else { return }

        let elapsedBuckets = Int((time - startedAt) / Self.bucketDuration)
        let bucketIndex = elapsedBuckets % Self.bucketCount
        if buckets[bucketIndex].index != elapsedBuckets {
            buckets[bucketIndex] = Bucket(index: elapsedBuckets, bytes: 0)
        }
        buckets[bucketIndex].bytes += totalBytes - self.totalBytes
        self.totalBytes = totalBytes
        latestAcceptedAt = time
    }

    func liveMbps(at now: Double) -> Double? {
        guard now.isFinite, startedAt.isFinite else { return nil }
        let elapsed = now - startedAt
        guard elapsed >= Self.warmupDuration else { return nil }
        guard let latestAcceptedAt, now >= latestAcceptedAt,
              now - latestAcceptedAt < Self.liveWindow else { return 0 }

        let currentBucket = Int(elapsed / Self.bucketDuration)
        let firstBucket = max(0, currentBucket - Self.bucketCount + 1)
        let recentBytes = buckets.reduce(0) { total, bucket in
            guard let index = bucket.index, index >= firstBucket, index <= currentBucket else { return total }
            return total + bucket.bytes
        }
        return Double(recentBytes) * 8 / 1_000_000 / min(elapsed, Self.liveWindow)
    }

    func averageMbps(at now: Double) -> Double? {
        guard now.isFinite, startedAt.isFinite else { return nil }
        let elapsed = now - startedAt
        guard elapsed > 0 else { return nil }
        return Double(totalBytes) * 8 / 1_000_000 / elapsed
    }
}
