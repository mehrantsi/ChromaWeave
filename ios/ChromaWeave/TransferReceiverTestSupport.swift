#if CW_DEVELOPMENT
import CryptoKit
import Foundation

extension TransferReceiver {
    static func selfTest() throws -> [String: Any] {
        let bytes = Data((0..<10_243).map { UInt8(truncatingIfNeeded: $0 * 31 + 7) })
        let transferID = Data((0..<16).map { UInt8($0 * 13 + 5) })
        let chunks = makeSourcePackets(bytes, transferID: transferID, chunkSize: 10_240)
        guard chunks.count == 2 else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        var outOfOrder = TransferReceiver()
        let last = try outOfOrder.accept(chunks[1])
        guard last.acceptedUnique, last.chunkIndex == 1, last.receivedChunks == 1, last.missingChunks == 1, last.missingIndices == [0],
              last.uniqueValidatedBytes == 3, last.completedData == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let completed = try outOfOrder.accept(chunks[0])
        guard completed.acceptedUnique, completed.chunkIndex == 0, completed.isComplete, completed.receivedChunks == 2,
              completed.missingChunks == 0, completed.missingIndices == [], completed.uniqueValidatedBytes == bytes.count,
              completed.completedData == bytes else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let duplicate = try outOfOrder.accept(chunks[0])
        guard duplicate.duplicate, !duplicate.acceptedUnique, duplicate.duplicateCount == 1,
              duplicate.completedData == nil, duplicate.uniqueValidatedBytes == bytes.count else {
            throw CWFailure(message: "The transfer receiver self-test failed.")
        }

        let goldenFile = Data((0..<20_483).map { UInt8($0 % 251) })
        let goldenFileHash = "5c273141ce94288fe8be6feeee7741db232a12e1b9409dc967127b0161ef1320"
        let goldenHeaders = [
            "43575400000102030405060708090a0b0c0d0e0f000003000028035000005c273141ce94288fe8be6feeee7741db232a12e1b9409dc967127b0161ef13201404",
            "43575400000102030405060708090a0b0c0d0e0f010003000028035000005c273141ce94288fe8be6feeee7741db232a12e1b9409dc967127b0161ef13201404",
            "43575400000102030405060708090a0b0c0d0e0f020003000028035000005c273141ce94288fe8be6feeee7741db232a12e1b9409dc967127b0161ef13201404"
        ]
        let goldenChunkHashes = [
            "de7bc9c26531c4e92c1c3982433877f4ef0fcffe9a5ab17cc48b3512a2496c11",
            "64f1ebf9d846d343211cc8aea7105107ccb6d32cb776b3e6983e807ee2ddbc49",
            "9a61b6373ed3280868e0db1a65a524ed5bb05609017977f4a13216f09c2ba346"
        ]
        guard Self.testHex(Data(SHA256.hash(data: goldenFile))) == goldenFileHash else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        var goldenChunks = [Data]()
        for index in 0..<goldenHeaders.count {
            guard let header = Self.data(hex: goldenHeaders[index]) else { throw CWFailure(message: "The transfer receiver self-test failed.") }
            let lower = index * 10_240
            let upper = min(goldenFile.count, lower + 10_240)
            var packet = header
            packet.append(goldenFile.subdata(in: lower..<upper))
            packet.append(Data(repeating: 0, count: 10_240 - (upper - lower)))
            guard Self.testHex(Data(SHA256.hash(data: packet))) == goldenChunkHashes[index] else {
                throw CWFailure(message: "The transfer receiver self-test failed.")
            }
            goldenChunks.append(packet)
        }
        var goldenReceiver = TransferReceiver()
        var goldenCompletion: Update?
        for index in [2, 0, 1] {
            let update = try goldenReceiver.accept(goldenChunks[index])
            guard update.acceptedUnique, update.expectedSHA256Hex == goldenFileHash else {
                throw CWFailure(message: "The transfer receiver self-test failed.")
            }
            if update.completedData != nil { goldenCompletion = update }
        }
        guard goldenReceiver.transferIDHex == "000102030405060708090a0b0c0d0e0f",
              goldenCompletion?.completedData == goldenFile,
              goldenCompletion?.uniqueValidatedBytes == goldenFile.count else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        var foreign = TransferReceiver()
        _ = try foreign.accept(chunks[0])
        let foreignID = foreign.transferIDHex
        try expectFailure(.foreignTransfer) {
            _ = try foreign.accept(makeSourcePackets(bytes, transferID: Data(repeating: 0xa5, count: 16), chunkSize: 10_240)[0])
        }
        guard foreign.transferIDHex == foreignID else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        var conflict = TransferReceiver()
        _ = try conflict.accept(chunks[0])
        var conflictingChunk = chunks[0]
        conflictingChunk[64] ^= 1
        try expectFailure(.conflictingDuplicate) { _ = try conflict.accept(conflictingChunk) }
        guard conflict.transferIDHex == Self.testHex(transferID),
              try conflict.accept(chunks[1]).completedData == bytes else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        var changedMetadataChunk = chunks[1]
        changedMetadataChunk[30] = changedMetadataChunk[30] ^ 1
        var metadataMismatch = TransferReceiver()
        _ = try metadataMismatch.accept(chunks[0])
        try expectFailure(.metadataMismatch) { _ = try metadataMismatch.accept(changedMetadataChunk) }
        guard metadataMismatch.transferIDHex == Self.testHex(transferID) else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        var truncated = TransferReceiver()
        try expectFailure(.truncatedHeader) { _ = try truncated.accept(chunks[0].prefix(63)) }
        try expectFailure(.truncatedChunk) { _ = try truncated.accept(chunks[0].dropLast()) }

        var badCount = chunks[0]
        badCount[22] = 0xff
        badCount[23] = 0xff
        var invalidCount = TransferReceiver()
        try expectFailure(.invalidChunkCount) { _ = try invalidCount.accept(badCount) }

        let maxTransferID = Data(repeating: 0x3c, count: 16)
        let maximumPacket = makeRepairPacket(bytes: Data(repeating: 1, count: 10_240), transferID: maxTransferID,
                                             index: 0, count: 3_277, chunkSize: 10_240,
                                             originalLength: maximumOriginalLength, digest: Data(repeating: 0, count: 32), groupData: 20)
        var maximum = TransferReceiver()
        let maximumProgress = try maximum.accept(maximumPacket)
        guard maximumProgress.totalChunks == 3_277, maximumProgress.missingChunks == 3_276,
              maximumProgress.uniqueValidatedBytes == 10_240 else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        let oversized = makeRepairPacket(bytes: Data(repeating: 1, count: 10_240), transferID: transferID,
                                         index: 0, count: 3_277, chunkSize: 10_240,
                                         originalLength: maximumOriginalLength + 1, digest: Data(repeating: 0, count: 32), groupData: 20)
        var invalidLength = TransferReceiver()
        try expectFailure(.invalidOriginalLength) { _ = try invalidLength.accept(oversized) }

        let badDigestChunks = makeSourcePackets(bytes, transferID: transferID, chunkSize: 10_240,
                                                 digest: Data(repeating: 0, count: 32))
        var badDigest = TransferReceiver()
        _ = try badDigest.accept(badDigestChunks[0])
        try expectFailure(.digestMismatch) { _ = try badDigest.accept(badDigestChunks[1]) }
        try expectFailure(.digestMismatch) { _ = try badDigest.accept(badDigestChunks[0]) }
        guard badDigest.transferIDHex == Self.testHex(transferID) else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        badDigest.reset()
        guard badDigest.transferIDHex == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        var rateMeter = TransferRateMeter(startedAt: 100)
        guard rateMeter.liveMbps(at: 100.249) == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        rateMeter.record(totalBytes: 100_000, at: 100.3)
        rateMeter.record(totalBytes: 100_000, at: 100.4)
        guard rateMeter.totalBytes == 100_000,
              rateMeter.liveMbps(at: 100.249) == nil,
              let warmRate = rateMeter.liveMbps(at: 100.5), abs(warmRate - 1.6) < 0.000_001,
              let initialAverage = rateMeter.averageMbps(at: 100.5), abs(initialAverage - 1.6) < 0.000_001 else {
            throw CWFailure(message: "The transfer receiver self-test failed.")
        }
        rateMeter.record(totalBytes: 200_000, at: 101)
        guard let recentRate = rateMeter.liveMbps(at: 101), abs(recentRate - 1.6) < 0.000_001,
              let expiredWindowRate = rateMeter.liveMbps(at: 102.3), abs(expiredWindowRate - 0.4) < 0.000_001,
              rateMeter.liveMbps(at: 103) == 0 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        rateMeter = TransferRateMeter(startedAt: 200)
        guard rateMeter.totalBytes == 0, rateMeter.liveMbps(at: 200.25) == 0,
              rateMeter.averageMbps(at: 200.25) == 0 else { throw CWFailure(message: "The transfer receiver self-test failed.") }

        return ["passed": true, "out_of_order": true, "duplicate_ignored": true,
                "foreign_transfer_rejected": true, "conflicting_duplicate_rejected": true,
                "metadata_mismatch_rejected": true,
                "count_overflow_rejected": true, "truncation_rejected": true,
                "maximum_length_accepted": true, "final_sha256_verified": true,
                "rust_golden_vector": true, "reset_clears_transfer": true,
                "rate_warmup": true, "rate_duplicate_ignored": true,
                "rate_window_expiry": true, "rate_idle_decay": true, "rate_reset": true,
                "unique_validated_bytes": bytes.count, "cwt": try repairSelfTest()]
    }

    static func repairSelfTest() throws -> [String: Any] {
        let size = 10_240
        let transferID = Data((0..<16).map(UInt8.init))
        let goldenFile = Data((0..<20_483).map { UInt8($0 % 251) })
        let goldenHeader = "43575400000102030405060708090a0b0c0d0e0f000003000028035000005c273141ce94288fe8be6feeee7741db232a12e1b9409dc967127b0161ef13201404"
        let goldenPacket = makeRepairPacket(bytes: goldenFile.prefix(size), transferID: transferID, index: 0, count: 3,
                                            chunkSize: size, originalLength: goldenFile.count,
                                            digest: Data(SHA256.hash(data: goldenFile)), groupData: 20)
        guard testHex(goldenPacket.prefix(64)) == goldenHeader else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let sources = (0..<3).map { shard in Data((0..<size).map { UInt8(truncatingIfNeeded: $0 * 73 + shard * 41) }) }
        let file = sources.reduce(into: Data()) { $0.append($1) }
        let patterns = [
            "a9f30a0fc24f2dd105a06839b9ff892456c25e4209ab02de34f425f25dd08615028f95a62659fdc5793fc1ddaf2f9d58c96bba54d9bf7e419d10332249ac19bce69945b25af266bf6fefd5a104b4e74e197fc6ff42c56891896c98b933bac9a89a32dec84c16975dc474afb7e04505e58205d01bb327c30af37a7c48d11152d28cd62f2ae76a08f420854d1c9cdaac0173e77b672c8e27fb11d100d778f5a33027aab083037cd8e05c1ae4f88a0ab87dec4e9f71fc9a5b64b83516076c893c99c3bc60977fd7439a4acaf0842191c26b3c5ae3da67e04db4ac49bd9c169fec8dbf17fbed6933b278e1518a92c56020c0a720f53e9602e62fd65f596df43477f7",
            "cdb114b770c4070779c83e680c760c5900e2e114be3a4b502a3d9da6f23a5b0af5412feaf2e3864589f363ea2bf74ea93bbf63333f78bb6b77bfba27b0ca60577766aea802db771aae72211a1306118ebafd930bce279cea354f82d6efede115875e5ff72559513796837e3d91203cb64ba2b489e80aa41b6a6800f0c2d5104aa0dc79da1da96a6a14a55305611b61346d8f8c79d357263d4750f0cb9f573667982c42879f8eeb28e49e0e87469a23c456d20e5e5215d6061ad2d74adda70d3a1a0bc3c56fb61a77c31f4c777e6b7ce3d790fe66a34af1875822efbb82808c78ea33329a48343c5afbee1350fc4d51db26cfd9e48567c97607056d9dafb87d27",
            "1b4168dd6b504292ed131e2bfcde8f3cbf65e8bc72c08051c9937f326c1c4c183f04f12cb0b824afa88aeff014b8b2792694335414fdc5c8384897545159d5e9e4ec9711f59c0a8840ecd2b53096959140a976703ada2dae050db37a76b1b3d4a1c8b9361d47893164c2f55deb152cb56e8e9eabb963098022e568f9cf959df349133a8f390210c0bf414c79ae8cdd6eed37baee2092d2039bc12d603e4e1e4a6d56a37ee2ea76fdfad8bda246eae02b74c6610646af979a6a1ac506030b87bbb6bec543a7ce58da12be80e762c4c7c312fb242268887ffc575fe12824e3e186f39aeb644f15db633690a70fb9477ee73cdcccf9eb315bd270b73aab9dc7cfa1",
            "04a3bb97c69f09c232b3bd600e3b83ef22b54aa8aac578092442820c544a48f9d38a26f2dbe65e091b2ed87d776c48c6bfd057d1fd0e5194415ffb5b9f63d59ccef37139f231baeb62791354a088aabfe81b7e0619ec28c38a762cbf7d1a8257e72495db8b2c0e3db59df12dbd3c7c680cf9071bad3aff27680f310babcd66b59e39210d5c059358a82927fa94a11975b82fd032305fe293bed81896ced0d2634910bc68417cc49381b442e7edf6d25c254acd4b6794cb0edbc561c105f94f065469eba368ab2071f8e389ce3a1230257281e49c8376b25910ecb625e78018cd7dbe0f4111b694a72f076bb727a6e6f296639d8137a065bdf295ab913157fc2f"
        ]
        let parity = try patterns.map { pattern -> Data in
            guard let pattern = data(hex: pattern), pattern.count == 256 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
            var result = Data()
            for _ in 0..<(size / 256) { result.append(pattern) }
            return result
        }
        let hashes = [
            "e35d23f00223d0866534484261672efbeb7de9b9b7cb9c6f7285b25277db1711",
            "5c71abf62f7fed4a37407cc03f11f6a52b111223c140c274dbacfd4b552c41d2",
            "0aca47bed64240d55d9d788015575990fc18ed3d383cc41f234a56063a342845",
            "cd55dfaa4ec6a53dd42bca8cc7a83fb60c08b21eec9b48e1f74b91e5870f2d47"
        ]
        for (bytes, hash) in zip(parity, hashes) {
            guard testHex(Data(SHA256.hash(data: bytes))) == hash else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        }
        let packets = (sources + parity).enumerated().map { index, bytes in
            makeRepairPacket(bytes: bytes, transferID: transferID, index: index, count: 3,
                             chunkSize: size, originalLength: file.count, digest: Data(SHA256.hash(data: file)), groupData: 20)
        }
        let fixtureRepair: Repair = { count, shards in
            guard count == 3, shards.count == 7 else { throw Failure.invalidRepairResult }
            for index in 0..<7 {
                if let bytes = shards[index], bytes != (sources + parity)[index] { throw Failure.invalidRepairResult }
            }
            return (0..<3).filter { shards[$0] == nil }.map { ($0, sources[$0]) }
        }
        var receiver = TransferReceiver(repair: fixtureRepair)
        let firstParity = try receiver.accept(packets[3])
        guard firstParity.acceptedRepairSymbol, !firstParity.acceptedUnique, firstParity.sourceChunkIndex == nil,
              firstParity.repairSymbols == 1, firstParity.uniqueValidatedBytes == 0,
              firstParity.receivedChunks == 0, !firstParity.isComplete else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        _ = try receiver.accept(packets[2])
        let completed = try receiver.accept(packets[5])
        guard completed.completedData == file, completed.isComplete, completed.envelope == "CWT",
              completed.acceptedUnique, completed.acceptedRepairSymbol, completed.repairedChunks == 2,
              completed.repairSymbols == 2, completed.receivedChunks == 3,
              completed.uniqueValidatedBytes == file.count else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let duplicate = try receiver.accept(packets[0])
        guard duplicate.duplicate, !duplicate.acceptedUnique, duplicate.repairedChunks == 2,
              duplicate.sourceChunkIndex == 0, duplicate.uniqueValidatedBytes == file.count else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let duplicateParity = try receiver.accept(packets[3])
        guard duplicateParity.duplicate, !duplicateParity.acceptedRepairSymbol, duplicateParity.repairSymbols == 2 else {
            throw CWFailure(message: "The transfer receiver self-test failed.")
        }
        _ = try receiver.accept(packets[4])
        var badParity = packets[6]
        badParity[64] ^= 1
        try expectFailure(.invalidRepairResult) { _ = try receiver.accept(badParity) }
        let validAfterRejection = try receiver.accept(packets[6])
        guard validAfterRejection.repairSymbols == 4, !validAfterRejection.acceptedUnique,
              validAfterRejection.completedData == nil, validAfterRejection.isComplete else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        var conflict = packets[0]
        conflict[64] ^= 1
        try expectFailure(.conflictingDuplicate) { _ = try receiver.accept(conflict) }
        var foreign = packets[0]
        foreign[4] ^= 1
        try expectFailure(.foreignTransfer) { _ = try receiver.accept(foreign) }
        var changed = packets[0]
        changed[30] ^= 1
        try expectFailure(.metadataMismatch) { _ = try receiver.accept(changed) }
        changed = packets[0]
        changed[62] = 60
        try expectFailure(.metadataMismatch) { _ = try receiver.accept(changed) }
        var invalidMagic = packets[0]
        invalidMagic[0] ^= 1
        var invalidEnvelope = TransferReceiver()
        try expectFailure(.invalidMagic) { _ = try invalidEnvelope.accept(invalidMagic) }
        guard invalidEnvelope.transferIDHex == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        var unsupported = TransferReceiver()
        _ = try unsupported.accept(packets[3])
        _ = try unsupported.accept(packets[0])
        try expectFailure(.unsupportedRepair) { _ = try unsupported.accept(packets[1]) }
        var sourceOnly = TransferReceiver()
        for index in [2, 0, 1] {
            let update = try sourceOnly.accept(packets[index])
            if index == 1, update.completedData != file { throw CWFailure(message: "The transfer receiver self-test failed.") }
        }
        let small = Data([1, 2, 3])
        var wideSource = small
        wideSource.append(Data(repeating: 0, count: 20_480 - small.count))
        var wide = TransferReceiver()
        let wideCompleted = try wide.accept(makeRepairPacket(bytes: wideSource, transferID: transferID, index: 0, count: 1,
                                                            chunkSize: 20_480, originalLength: small.count,
                                                            digest: Data(SHA256.hash(data: small)), groupData: 60))
        guard wideCompleted.completedData == small, wideCompleted.uniqueValidatedBytes == small.count,
              wideCompleted.repairSymbols == 0 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        var maximum = TransferReceiver()
        let maximumProgress = try maximum.accept(makeRepairPacket(bytes: sources[0], transferID: transferID, index: 0, count: 3_277,
                                                                  chunkSize: size, originalLength: maximumOriginalLength,
                                                                  digest: Data(repeating: 0, count: 32), groupData: 60))
        guard maximumProgress.totalChunks == 3_277, maximumProgress.receivedChunks == 1 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        var invalidLength = TransferReceiver()
        for length in [0, maximumOriginalLength + 1] {
            try expectFailure(.invalidOriginalLength) {
                _ = try invalidLength.accept(makeRepairPacket(bytes: sources[0], transferID: transferID, index: 0, count: 3_277,
                                                              chunkSize: size, originalLength: length,
                                                              digest: Data(repeating: 0, count: 32), groupData: 20))
            }
        }
        for (offset, value, failure) in [(62, UInt8(0), Failure.invalidRepairHeader), (62, 21, .invalidRepairHeader),
                                        (63, 3, .invalidRepairHeader), (22, 4, .invalidChunkCount),
                                        (20, 7, .invalidChunkIndex)] {
            var invalid = packets[0]
            invalid[offset] = value
            var fresh = TransferReceiver(repair: fixtureRepair)
            try expectFailure(failure) { _ = try fresh.accept(invalid) }
        }
        var truncated = TransferReceiver()
        try expectFailure(.truncatedHeader) { _ = try truncated.accept(packets[0].prefix(63)) }
        try expectFailure(.truncatedChunk) { _ = try truncated.accept(packets[0].dropLast()) }
        var overlong = packets[0]
        overlong.append(0)
        try expectFailure(.invalidChunkLength) { _ = try truncated.accept(overlong) }
        var badDigest = TransferReceiver()
        for index in [0, 1] {
            var packet = packets[index]
            packet[30] ^= 1
            _ = try badDigest.accept(packet)
        }
        var last = packets[2]
        last[30] ^= 1
        try expectFailure(.digestMismatch) { _ = try badDigest.accept(last) }
        try expectFailure(.digestMismatch) { _ = try badDigest.accept(last) }

        let tailFile = Data((0..<(21 * size + 17)).map { UInt8(truncatingIfNeeded: $0 * 31 + 7) })
        let tailSources = (0..<22).map { index -> Data in
            let start = index * size
            var bytes = tailFile.subdata(in: start..<min(tailFile.count, start + size))
            bytes.append(Data(repeating: 0, count: size - bytes.count))
            return bytes
        }
        func groupParity(_ group: Int) -> [Data] {
            (0..<4).map { Data(repeating: UInt8(0xc0 + group * 4 + $0), count: size) }
        }
        let groupRepair: Repair = { count, shards in
            guard count == 20 || count == 2, shards.count == count + 4 else { throw Failure.invalidRepairResult }
            let group = count == 20 ? 0 : 1
            let known = Array(tailSources[(group * 20)..<(group * 20 + count)]) + groupParity(group)
            for index in shards.indices {
                if let bytes = shards[index], bytes != known[index] { throw Failure.invalidRepairResult }
            }
            return (0..<count).filter { shards[$0] == nil }.map { ($0, known[$0]) }
        }
        func tailPacket(_ group: Int, _ local: Int) -> Data {
            let count = group == 0 ? 20 : 2
            let bytes = local < count ? tailSources[group * 20 + local] : groupParity(group)[local - count]
            return makeRepairPacket(bytes: bytes, transferID: transferID, index: group * 24 + local, count: 22,
                                    chunkSize: size, originalLength: tailFile.count, digest: Data(SHA256.hash(data: tailFile)), groupData: 20)
        }
        var tooManyMissing = TransferReceiver(repair: groupRepair)
        var lastMissingUpdate: Update?
        for local in 0..<24 where ![0, 3, 7, 14, 19].contains(local) {
            let update = try tooManyMissing.accept(tailPacket(0, local))
            lastMissingUpdate = update
            guard update.repairedChunks == 0, !update.isComplete, update.completedData == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        }
        guard lastMissingUpdate?.receivedChunks == 15, lastMissingUpdate?.repairSymbols == 4 else {
            throw CWFailure(message: "The transfer receiver self-test failed.")
        }
        var grouped = TransferReceiver(repair: groupRepair)
        _ = try grouped.accept(tailPacket(1, 1))
        let partial = try grouped.accept(tailPacket(1, 2))
        guard partial.repairedChunks == 1, partial.uniqueValidatedBytes == size + 17,
              partial.receivedChunks == 2, partial.completedData == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        for local in 0..<24 where ![0, 7, 14, 19, 23].contains(local) {
            let update = try grouped.accept(tailPacket(0, local))
            guard update.completedData == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        }
        guard try grouped.accept(tailPacket(0, 22)).receivedChunks == 18 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let groupComplete = try grouped.accept(tailPacket(0, 23))
        guard groupComplete.completedData == tailFile, groupComplete.repairedChunks == 5,
              groupComplete.uniqueValidatedBytes == tailFile.count, groupComplete.receivedChunks == 22 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        var badPadding = tailPacket(1, 1)
        badPadding[badPadding.count - 1] = 1
        var padding = TransferReceiver(repair: groupRepair)
        try expectFailure(.invalidSourcePadding) { _ = try padding.accept(badPadding) }
        let invalidPaddingRepair: Repair = { count, shards in
            var result = try groupRepair(count, shards)
            for index in result.indices where result[index].index == 1 {
                result[index].data[size - 1] = 1
            }
            return result
        }
        var restoredPadding = TransferReceiver(repair: invalidPaddingRepair)
        _ = try restoredPadding.accept(tailPacket(1, 2))
        try expectFailure(.invalidSourcePadding) { _ = try restoredPadding.accept(tailPacket(1, 3)) }
        let retainedPadding = try restoredPadding.accept(tailPacket(1, 2))
        guard retainedPadding.receivedChunks == 0, retainedPadding.repairSymbols == 1 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        let invalidResult: Repair = { _, _ in [(0, Data(repeating: 0, count: size))] }
        var invalid = TransferReceiver(repair: invalidResult)
        _ = try invalid.accept(packets[3])
        _ = try invalid.accept(packets[4])
        try expectFailure(.invalidRepairResult) { _ = try invalid.accept(packets[5]) }
        let retainedInvalid = try invalid.accept(packets[3])
        guard retainedInvalid.receivedChunks == 0, retainedInvalid.repairSymbols == 2 else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        receiver.reset()
        guard receiver.transferIDHex == nil else { throw CWFailure(message: "The transfer receiver self-test failed.") }
        return ["passed": true, "native_parity_golden": true, "rust_header_golden": true,
                "both_chunk_sizes_and_group_sizes": true, "canonical_bounds": true,
                "parity_first_accounting": true,
                "missing_sources_repaired": true, "repaired_source_duplicate": true,
                "all_supplied_parity_checked": true, "invalid_repair_atomic": true,
                "partial_group_and_four_erasures": true, "too_many_erasures_wait": true,
                "source_and_recovered_padding": true, "final_sha256": true,
                "unsupported_repair_rejected": true, "invalid_magic_rejected": true, "source_only_supported": true,
                "flow_repair_is_fixture_stub": true]
    }

    private static func makeRepairPacket(bytes: Data, transferID: Data, index: Int, count: Int,
                                         chunkSize: Int, originalLength: Int, digest: Data, groupData: Int) -> Data {
        var packet = Data("CWT\0".utf8)
        packet.append(transferID)
        appendUInt16(UInt16(index), to: &packet)
        appendUInt16(UInt16(count), to: &packet)
        appendUInt16(UInt16(chunkSize), to: &packet)
        appendUInt32(UInt32(originalLength), to: &packet)
        packet.append(digest)
        packet.append(UInt8(groupData))
        packet.append(4)
        packet.append(bytes)
        return packet
    }

    private static func data(hex: String) -> Data? {
        guard hex.count.isMultiple(of: 2) else { return nil }
        var result = Data()
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
            result.append(byte)
            index = next
        }
        return result
    }

    private static func makeSourcePackets(_ data: Data, transferID: Data, chunkSize: Int,
                                          digest: Data? = nil) -> [Data] {
        let count = (data.count + chunkSize - 1) / chunkSize
        let checksum = digest ?? Data(SHA256.hash(data: data))
        return (0..<count).map { index in
            let lower = index * chunkSize
            let upper = min(data.count, lower + chunkSize)
            var source = data.subdata(in: lower..<upper)
            source.append(Data(repeating: 0, count: chunkSize - source.count))
            return makeRepairPacket(bytes: source, transferID: transferID,
                                    index: index / 20 * 24 + index % 20, count: count, chunkSize: chunkSize,
                                    originalLength: data.count, digest: checksum, groupData: 20)
        }
    }

    private static func appendUInt16(_ value: UInt16, to data: inout Data) {
        data.append(UInt8(truncatingIfNeeded: value))
        data.append(UInt8(truncatingIfNeeded: value >> 8))
    }

    private static func appendUInt32(_ value: UInt32, to data: inout Data) {
        data.append(UInt8(truncatingIfNeeded: value))
        data.append(UInt8(truncatingIfNeeded: value >> 8))
        data.append(UInt8(truncatingIfNeeded: value >> 16))
        data.append(UInt8(truncatingIfNeeded: value >> 24))
    }

    private static func expectFailure(_ expected: Failure, operation: () throws -> Void) throws {
        do {
            try operation()
        } catch let failure as Failure {
            guard failure == expected else { throw CWFailure(message: "The transfer receiver self-test failed.") }
            return
        } catch {
            throw CWFailure(message: "The transfer receiver self-test failed.")
        }
        throw CWFailure(message: "The transfer receiver self-test failed.")
    }    
    private static func testHex(_ data: Data) -> String { data.map { String(format: "%02x", $0) }.joined() }
}
#endif
