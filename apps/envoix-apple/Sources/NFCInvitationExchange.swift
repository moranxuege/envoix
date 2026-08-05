#if os(iOS) && canImport(CoreNFC)
import Combine
@preconcurrency import CoreNFC
import Foundation
import OSLog

enum NFCInvitationContractError: Error, Equatable, LocalizedError {
    case messageCount
    case recordCount
    case recordType
    case malformedURI
    case malformedCarrier
    case unsupportedInvitation
    case oversizedInvitation

    var errorDescription: String? {
        switch self {
        case .messageCount:
            return "The NFC tag must contain exactly one NDEF message."
        case .recordCount:
            return "The NFC tag must contain exactly one URI record."
        case .recordType:
            return "The NFC tag does not contain an Envoix URI record."
        case .malformedURI:
            return "The NFC invitation is not valid printable ASCII."
        case .malformedCarrier:
            return "The NFC invitation link is malformed."
        case .unsupportedInvitation:
            return "The NFC tag does not contain an Envoix invitation."
        case .oversizedInvitation:
            return "The NFC invitation is too large."
        }
    }
}

enum NFCInvitationNDEFCodec {
    static let maximumInvitationBytes = 8_211
    static let carrierPrefix = "https://ece4410j-nuub.github.io/nfc/v1/#"

    private static let uriType = Data([0x55])
    private static let uncompressedURIPrefix: UInt8 = 0x00
    private static let invitationPrefixes = [
        "envoix://invite/v2/",
        "envoix://room/"
    ]
    private static let maximumEncodedInvitationCharacters =
        ((maximumInvitationBytes + 2) / 3) * 4
    private static let maximumCarrierBytes =
        carrierPrefix.utf8.count + maximumEncodedInvitationCharacters
    // One long-form URI record: header, type length, four-byte payload
    // length, one-byte type, URI prefix byte, and the carrier URI.
    static let maximumSerializedMessageBytes = maximumCarrierBytes + 8

    static func message(for invitation: String) throws -> NFCNDEFMessage {
        let bytes = Data(try carrierURI(for: invitation).utf8)
        let record = NFCNDEFPayload(
            format: .nfcWellKnown,
            type: uriType,
            identifier: Data(),
            payload: Data([uncompressedURIPrefix]) + bytes
        )
        return NFCNDEFMessage(records: [record])
    }

    static func invitation(from messages: [NFCNDEFMessage]) throws -> String {
        guard messages.count == 1, let message = messages.first else {
            throw NFCInvitationContractError.messageCount
        }
        return try invitation(from: message)
    }

    static func invitation(from message: NFCNDEFMessage) throws -> String {
        guard message.records.count == 1, let record = message.records.first else {
            throw NFCInvitationContractError.recordCount
        }
        guard record.typeNameFormat == .nfcWellKnown,
              record.type == uriType,
              record.identifier.isEmpty,
              record.payload.first == uncompressedURIPrefix else {
            throw NFCInvitationContractError.recordType
        }
        let uriBytes = Data(record.payload.dropFirst())
        guard uriBytes.count <= maximumCarrierBytes else {
            throw NFCInvitationContractError.oversizedInvitation
        }
        guard let uri = String(data: uriBytes, encoding: .ascii),
              uri.utf8.count == uriBytes.count,
              uriBytes.allSatisfy({ 0x21...0x7e ~= $0 }) else {
            throw NFCInvitationContractError.malformedURI
        }
        if invitationPrefixes.contains(where: uri.hasPrefix) {
            _ = try validatedInvitationASCII(uri)
            return uri
        }
        return try invitation(fromCarrierURI: uri)
    }

    static func carrierURL(for invitation: String) throws -> URL {
        let uri = try carrierURI(for: invitation)
        guard let url = URL(string: uri) else {
            throw NFCInvitationContractError.malformedCarrier
        }
        return url
    }

    static func invitation(fromCarrierURL url: URL) throws -> String {
        try invitation(fromCarrierURI: url.absoluteString)
    }

    static func invitation(fromDirectURL url: URL) throws -> String {
        let invitation = url.absoluteString
        _ = try validatedInvitationASCII(invitation)
        return invitation
    }

    private static func carrierURI(for invitation: String) throws -> String {
        let invitationBytes = try validatedInvitationASCII(invitation)
        let encoded = invitationBytes.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        return carrierPrefix + encoded
    }

    private static func invitation(fromCarrierURI uri: String) throws -> String {
        guard uri.hasPrefix(carrierPrefix) else {
            throw NFCInvitationContractError.unsupportedInvitation
        }
        guard uri.utf8.count <= maximumCarrierBytes else {
            throw NFCInvitationContractError.oversizedInvitation
        }

        let encoded = String(uri.dropFirst(carrierPrefix.count))
        guard !encoded.isEmpty,
              encoded.utf8.count <= maximumEncodedInvitationCharacters,
              encoded.utf8.allSatisfy({
                  (0x41...0x5a).contains($0)
                      || (0x61...0x7a).contains($0)
                      || (0x30...0x39).contains($0)
                      || $0 == 0x2d
                      || $0 == 0x5f
              }),
              encoded.utf8.count % 4 != 1 else {
            throw NFCInvitationContractError.malformedCarrier
        }

        var base64 = encoded
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        base64 += String(
            repeating: "=",
            count: (4 - base64.utf8.count % 4) % 4
        )
        guard let decoded = Data(base64Encoded: base64),
              decoded.count <= maximumInvitationBytes,
              base64URLString(for: decoded) == encoded,
              let invitation = String(data: decoded, encoding: .ascii),
              invitation.utf8.count == decoded.count else {
            throw NFCInvitationContractError.malformedCarrier
        }
        _ = try validatedInvitationASCII(invitation)
        return invitation
    }

    private static func validatedInvitationASCII(_ invitation: String) throws -> Data {
        guard let bytes = invitation.data(using: .ascii),
              bytes.count == invitation.utf8.count else {
            throw NFCInvitationContractError.malformedURI
        }
        guard bytes.count <= maximumInvitationBytes else {
            throw NFCInvitationContractError.oversizedInvitation
        }
        guard bytes.allSatisfy({ 0x21...0x7e ~= $0 }),
              invitationPrefixes.contains(where: { prefix in
                  invitation.hasPrefix(prefix)
                      && bytes.count > prefix.utf8.count
              }) else {
            throw NFCInvitationContractError.unsupportedInvitation
        }
        return bytes
    }

    private static func base64URLString(for data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

enum NFCInvitationExchangeError: Error, LocalizedError {
    case unavailable
    case busy

    var errorDescription: String? {
        switch self {
        case .unavailable:
            return "NFC tag reading is not available on this device."
        case .busy:
            return "Another NFC operation is already active."
        }
    }
}

enum NFCInvitationReadResult {
    case success(String)
    case cancelled
    case failure(Error)
}

enum NFCInvitationPrivateAIDError: Error, Equatable, LocalizedError {
    case unsupportedTag
    case unexpectedApplication
    case invalidCommand
    case commandFailed(status: UInt16)
    case unexpectedResponseLength(expected: Int, actual: Int)
    case invalidNDEFLength
    case malformedNDEFMessage

    var errorDescription: String? {
        switch self {
        case .unsupportedTag:
            return "This is not a compatible Envoix phone."
        case .unexpectedApplication:
            return "The phone did not select the Envoix NFC service."
        case .invalidCommand:
            return "The Envoix NFC read request is invalid."
        case .commandFailed(let status):
            return String(
                format: "The Envoix phone rejected the NFC read (status %04X).",
                status
            )
        case .unexpectedResponseLength:
            return "The Envoix phone returned an incomplete NFC response."
        case .invalidNDEFLength:
            return "The Envoix phone returned an invalid invitation size."
        case .malformedNDEFMessage:
            return "The Envoix phone returned a malformed NFC invitation."
        }
    }
}

struct NFCInvitationISO7816Command: Equatable {
    let bytes: Data
    let expectedResponseLength: Int

    var apdu: NFCISO7816APDU? {
        NFCISO7816APDU(data: bytes)
    }
}

enum NFCInvitationPrivateAIDProtocol {
    static let applicationIdentifier = "F0454E564F495801"
    static let maximumReadBytes = 0xff

    static let selectNDEFFile = NFCInvitationISO7816Command(
        bytes: Data([0x00, 0xa4, 0x00, 0x0c, 0x02, 0xe1, 0x04]),
        expectedResponseLength: 0
    )
    static let readNDEFLength = NFCInvitationISO7816Command(
        bytes: Data([0x00, 0xb0, 0x00, 0x00, 0x02]),
        expectedResponseLength: 2
    )

    static func matchesApplicationIdentifier(_ value: String) -> Bool {
        value.caseInsensitiveCompare(applicationIdentifier) == .orderedSame
    }

    static func readBinary(offset: Int, length: Int) throws
        -> NFCInvitationISO7816Command {
        guard (0...0xffff).contains(offset),
              (1...maximumReadBytes).contains(length) else {
            throw NFCInvitationPrivateAIDError.invalidCommand
        }
        return NFCInvitationISO7816Command(
            bytes: Data([
                0x00,
                0xb0,
                UInt8((offset >> 8) & 0xff),
                UInt8(offset & 0xff),
                UInt8(length)
            ]),
            expectedResponseLength: length
        )
    }

    static func validateResponse(
        _ data: Data,
        sw1: UInt8,
        sw2: UInt8,
        for command: NFCInvitationISO7816Command
    ) throws {
        let status = UInt16(sw1) << 8 | UInt16(sw2)
        guard status == 0x9000 else {
            throw NFCInvitationPrivateAIDError.commandFailed(status: status)
        }
        guard data.count == command.expectedResponseLength else {
            throw NFCInvitationPrivateAIDError.unexpectedResponseLength(
                expected: command.expectedResponseLength,
                actual: data.count
            )
        }
    }

    static func ndefLength(from data: Data) throws -> Int {
        guard data.count == 2 else {
            throw NFCInvitationPrivateAIDError.unexpectedResponseLength(
                expected: 2,
                actual: data.count
            )
        }
        let length = Int(data[data.startIndex]) << 8
            | Int(data[data.index(after: data.startIndex)])
        guard (1...NFCInvitationNDEFCodec.maximumSerializedMessageBytes)
            .contains(length) else {
            throw NFCInvitationPrivateAIDError.invalidNDEFLength
        }
        return length
    }

    static func message(from data: Data) throws -> NFCNDEFMessage {
        guard (1...NFCInvitationNDEFCodec.maximumSerializedMessageBytes)
            .contains(data.count) else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }
        try validateSingleRecordEnvelope(data)
        guard let message = NFCNDEFMessage(data: data) else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }
        return message
    }

    private static func validateSingleRecordEnvelope(_ data: Data) throws {
        let bytes = [UInt8](data)
        guard bytes.count >= 3 else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }

        let header = bytes[0]
        let hasMessageBegin = header & 0x80 != 0
        let hasMessageEnd = header & 0x40 != 0
        let isChunked = header & 0x20 != 0
        let isShortRecord = header & 0x10 != 0
        let hasIdentifierLength = header & 0x08 != 0
        let typeNameFormat = header & 0x07
        guard hasMessageBegin,
              hasMessageEnd,
              !isChunked,
              typeNameFormat == 0x01 else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }

        let typeLength = Int(bytes[1])
        var cursor = 2
        let payloadLength: Int
        if isShortRecord {
            guard cursor < bytes.count else {
                throw NFCInvitationPrivateAIDError.malformedNDEFMessage
            }
            payloadLength = Int(bytes[cursor])
            cursor += 1
        } else {
            guard bytes.count - cursor >= 4 else {
                throw NFCInvitationPrivateAIDError.malformedNDEFMessage
            }
            payloadLength = Int(
                UInt32(bytes[cursor]) << 24
                    | UInt32(bytes[cursor + 1]) << 16
                    | UInt32(bytes[cursor + 2]) << 8
                    | UInt32(bytes[cursor + 3])
            )
            cursor += 4
        }

        var identifierLength = 0
        if hasIdentifierLength {
            guard cursor < bytes.count else {
                throw NFCInvitationPrivateAIDError.malformedNDEFMessage
            }
            identifierLength = Int(bytes[cursor])
            cursor += 1
        }

        guard typeLength <= bytes.count - cursor else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }
        cursor += typeLength
        guard identifierLength <= bytes.count - cursor else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }
        cursor += identifierLength
        guard payloadLength == bytes.count - cursor else {
            throw NFCInvitationPrivateAIDError.malformedNDEFMessage
        }
    }
}

struct NFCInvitationTerminalGate<Value> {
    private var stagedValue: Value?

    mutating func stage(_ value: Value) -> Bool {
        guard stagedValue == nil else { return false }
        stagedValue = value
        return true
    }

    mutating func take() -> Value? {
        defer { stagedValue = nil }
        return stagedValue
    }
}

private struct NFCInvitationSessionIdentity: Equatable, Sendable {
    private let objectIdentifier: ObjectIdentifier

    init(_ session: NFCTagReaderSession) {
        objectIdentifier = ObjectIdentifier(session)
    }
}

@MainActor
private final class NFCInvitationISO7816TagReference {
    let detectedTag: NFCTag
    let tag: NFCISO7816Tag

    init?(detectedTag: NFCTag) {
        guard case .iso7816(let tag) = detectedTag else { return nil }
        self.detectedTag = detectedTag
        self.tag = tag
    }
}

@MainActor
private final class NFCInvitationPrivateAIDReadContext {
    let tag: NFCISO7816Tag
    let expectedLength: Int
    var messageData: Data

    init(tag: NFCISO7816Tag, expectedLength: Int) {
        self.tag = tag
        self.expectedLength = expectedLength
        messageData = Data()
        messageData.reserveCapacity(expectedLength)
    }
}

@MainActor
private final class NFCInvitationDeferredDelivery {
    private var action: (() -> Void)?

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    func deliver() {
        let action = action
        self.action = nil
        action?()
    }
}

enum NFCInvitationTagLossRetryPolicy {
    static let maximumAttempts = 3

    static func shouldRetry(
        code: NFCReaderError.Code?,
        completedAttempts: Int
    ) -> Bool {
        guard completedAttempts >= 0,
              completedAttempts < maximumAttempts,
              let code else {
            return false
        }
        switch code {
        case .readerTransceiveErrorTagConnectionLost,
             .readerTransceiveErrorRetryExceeded,
             .readerTransceiveErrorTagNotConnected:
            return true
        default:
            return false
        }
    }
}

@MainActor
final class NFCInvitationExchange: NSObject, ObservableObject {
    static var isAvailable: Bool { NFCReaderSession.readingAvailable }

    private static let logger = Logger(
        subsystem: "com.envoix.app.ios",
        category: "nfc-invitation"
    )

    @Published private(set) var isActive = false

    private var completion: ((NFCInvitationReadResult) -> Void)?
    private var phoneSession: NFCTagReaderSession?
    private var terminalDelivery = NFCInvitationTerminalGate<() -> Void>()
    private var readingTimeout: DispatchWorkItem?
    private var completedTagLossRetryAttempts = 0
    private var activePrompt: String?

    func beginReadingEnvoixPhone(
        prompt: String,
        timeout: TimeInterval? = nil,
        completion: @escaping (NFCInvitationReadResult) -> Void
    ) {
        guard Self.isAvailable else {
            completion(.failure(NFCInvitationExchangeError.unavailable))
            return
        }
        guard self.completion == nil else {
            completion(.failure(NFCInvitationExchangeError.busy))
            return
        }
        guard let session = NFCTagReaderSession(
            pollingOption: .iso14443,
            delegate: self,
            queue: .main
        ) else {
            completion(.failure(NFCInvitationExchangeError.unavailable))
            return
        }
        self.completion = completion
        phoneSession = session
        completedTagLossRetryAttempts = 0
        activePrompt = prompt
        isActive = true
        session.alertMessage = prompt
        Self.logger.debug("Starting Envoix ISO 7816 reader session")
        session.begin()
        if let timeout, timeout > 0 {
            let work = DispatchWorkItem { [weak self, weak session] in
                MainActor.assumeIsolated {
                    guard let self, let session,
                          self.phoneSession === session else {
                        return
                    }
                    self.cancelReading()
                }
            }
            readingTimeout = work
            DispatchQueue.main.asyncAfter(
                deadline: .now() + timeout,
                execute: work
            )
        }
    }

    func cancelReading() {
        guard let session = phoneSession,
              let completion,
              terminalDelivery.stage({
                  completion(.cancelled)
              }) else { return }
        session.invalidate()
    }

    private func currentPhoneSession(
        matching identity: NFCInvitationSessionIdentity
    ) -> NFCTagReaderSession? {
        guard let phoneSession,
              NFCInvitationSessionIdentity(phoneSession) == identity else {
            return nil
        }
        return phoneSession
    }

    private func fail(_ error: Error, in session: NFCTagReaderSession) {
        guard phoneSession === session else { return }
        let code = (error as? NFCReaderError)?.code
        if NFCInvitationTagLossRetryPolicy.shouldRetry(
            code: code,
            completedAttempts: completedTagLossRetryAttempts
        ) {
            completedTagLossRetryAttempts += 1
            Self.logger.notice(
                "NFC target connection lost; restarting polling attempt \(self.completedTagLossRetryAttempts, privacy: .public)"
            )
            if let activePrompt {
                session.alertMessage = activePrompt
            }
            session.restartPolling()
            return
        }
        guard let completion,
              terminalDelivery.stage({
                  completion(.failure(error))
              }) else { return }
        Self.logger.error(
            "NFC invitation read failed with code \((code?.rawValue ?? -1), privacy: .public)"
        )
        session.invalidate(errorMessage: error.localizedDescription)
    }

    private func finishPhoneRead(
        _ invitation: String,
        in session: NFCTagReaderSession
    ) {
        guard phoneSession === session,
              let completion else { return }
        guard terminalDelivery.stage({
            completion(.success(invitation))
        }) else { return }
        session.alertMessage = "Envoix invitation found. Confirm in the app."
        session.invalidate()
    }

    private func readPrivateAID(
        _ tag: NFCISO7816Tag,
        in session: NFCTagReaderSession
    ) {
        guard NFCInvitationPrivateAIDProtocol.matchesApplicationIdentifier(
            tag.initialSelectedAID
        ) else {
            fail(NFCInvitationPrivateAIDError.unexpectedApplication, in: session)
            return
        }
        let identity = NFCInvitationSessionIdentity(session)
        send(
            NFCInvitationPrivateAIDProtocol.selectNDEFFile,
            to: tag,
            sessionIdentity: identity
        ) { [weak self] result in
            guard let self,
                  let session = self.currentPhoneSession(matching: identity) else {
                return
            }
            switch result {
            case .success:
                self.readPrivateAIDLength(tag, in: session)
            case .failure(let error):
                self.fail(error, in: session)
            }
        }
    }

    private func readPrivateAIDLength(
        _ tag: NFCISO7816Tag,
        in session: NFCTagReaderSession
    ) {
        let identity = NFCInvitationSessionIdentity(session)
        send(
            NFCInvitationPrivateAIDProtocol.readNDEFLength,
            to: tag,
            sessionIdentity: identity
        ) { [weak self] result in
            guard let self,
                  let session = self.currentPhoneSession(matching: identity) else {
                return
            }
            do {
                let data = try result.get()
                let length = try NFCInvitationPrivateAIDProtocol.ndefLength(
                    from: data
                )
                self.readPrivateAIDChunk(
                    NFCInvitationPrivateAIDReadContext(
                        tag: tag,
                        expectedLength: length
                    ),
                    in: session
                )
            } catch {
                self.fail(error, in: session)
            }
        }
    }

    private func readPrivateAIDChunk(
        _ context: NFCInvitationPrivateAIDReadContext,
        in session: NFCTagReaderSession
    ) {
        let remaining = context.expectedLength - context.messageData.count
        guard remaining > 0 else {
            do {
                finishPhoneRead(
                    try NFCInvitationNDEFCodec.invitation(
                        from: NFCInvitationPrivateAIDProtocol.message(
                            from: context.messageData
                        )
                    ),
                    in: session
                )
            } catch {
                fail(error, in: session)
            }
            return
        }

        let chunkLength = min(
            remaining,
            NFCInvitationPrivateAIDProtocol.maximumReadBytes
        )
        let command: NFCInvitationISO7816Command
        do {
            command = try NFCInvitationPrivateAIDProtocol.readBinary(
                offset: 2 + context.messageData.count,
                length: chunkLength
            )
        } catch {
            fail(error, in: session)
            return
        }

        let identity = NFCInvitationSessionIdentity(session)
        send(command, to: context.tag, sessionIdentity: identity) {
            [weak self, context] result in
            guard let self,
                  let session = self.currentPhoneSession(matching: identity) else {
                return
            }
            do {
                context.messageData.append(try result.get())
                self.readPrivateAIDChunk(context, in: session)
            } catch {
                self.fail(error, in: session)
            }
        }
    }

    private func send(
        _ command: NFCInvitationISO7816Command,
        to tag: NFCISO7816Tag,
        sessionIdentity: NFCInvitationSessionIdentity,
        completion: @escaping (Result<Data, Error>) -> Void
    ) {
        guard let apdu = command.apdu else {
            completion(.failure(NFCInvitationPrivateAIDError.invalidCommand))
            return
        }
        tag.sendCommand(apdu: apdu) {
            [weak self, sessionIdentity] data, sw1, sw2, error in
            MainActor.assumeIsolated {
                guard let self,
                      self.currentPhoneSession(matching: sessionIdentity) != nil else {
                    return
                }
                if let error {
                    completion(.failure(error))
                    return
                }
                do {
                    try NFCInvitationPrivateAIDProtocol.validateResponse(
                        data,
                        sw1: sw1,
                        sw2: sw2,
                        for: command
                    )
                    completion(.success(data))
                } catch {
                    completion(.failure(error))
                }
            }
        }
    }
}

extension NFCInvitationExchange: @preconcurrency NFCTagReaderSessionDelegate {
    func tagReaderSessionDidBecomeActive(_ session: NFCTagReaderSession) {}

    func tagReaderSession(
        _ session: NFCTagReaderSession,
        didDetect tags: [NFCTag]
    ) {
        guard phoneSession === session else { return }
        Self.logger.debug("Detected \(tags.count, privacy: .public) NFC target(s)")
        guard tags.count == 1, let detectedTag = tags.first else {
            session.alertMessage =
                "More than one NFC device was detected. Try again with one Envoix phone."
            let identity = NFCInvitationSessionIdentity(session)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                [weak self] in
                MainActor.assumeIsolated {
                    self?.currentPhoneSession(matching: identity)?
                        .restartPolling()
                }
            }
            return
        }
        guard let reference = NFCInvitationISO7816TagReference(
            detectedTag: detectedTag
        ) else {
            fail(NFCInvitationPrivateAIDError.unsupportedTag, in: session)
            return
        }
        guard NFCInvitationPrivateAIDProtocol.matchesApplicationIdentifier(
            reference.tag.initialSelectedAID
        ) else {
            fail(NFCInvitationPrivateAIDError.unexpectedApplication, in: session)
            return
        }

        let identity = NFCInvitationSessionIdentity(session)
        session.connect(to: reference.detectedTag) {
            [weak self, identity, reference] error in
            MainActor.assumeIsolated {
                guard let self,
                      let session = self.currentPhoneSession(
                          matching: identity
                      ) else { return }
                if let error {
                    self.fail(error, in: session)
                    return
                }
                Self.logger.debug("Connected to Envoix ISO 7816 target")
                self.readPrivateAID(reference.tag, in: session)
            }
        }
    }

    func tagReaderSession(
        _ session: NFCTagReaderSession,
        didInvalidateWithError error: Error
    ) {
        guard phoneSession === session else { return }
        readingTimeout?.cancel()
        readingTimeout = nil
        let completion = self.completion
        var delivery = terminalDelivery.take()
        self.completion = nil
        phoneSession = nil
        completedTagLossRetryAttempts = 0
        activePrompt = nil
        isActive = false
        if delivery == nil, let completion {
            delivery = Self.isUserCancellation(error)
                ? { completion(.cancelled) }
                : { completion(.failure(error)) }
        }
        if let delivery {
            let deferredDelivery = NFCInvitationDeferredDelivery(delivery)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    deferredDelivery.deliver()
                }
            }
        }
    }

    private static func isUserCancellation(_ error: Error) -> Bool {
        guard let readerError = error as? NFCReaderError else { return false }
        return readerError.code == .readerSessionInvalidationErrorUserCanceled
    }
}
#endif
