// Proton's security keys, held to the top of the iPhone (NFC), for Rust
// (src/ios/keys.rs). A Core NFC tag session looks for an ISO 7816 tag that
// answers to the FIDO applet (Info.plist's select identifiers list it) and
// moves APDUs to and from it; CTAP2 is Rust's.
//
// Rust calls in from its own thread and waits: the session's work runs on a
// queue of its own, and each exchange is waited for here.

import CoreNFC
import Foundation

final class PicachoSecurityKeys: NSObject, NFCTagReaderSessionDelegate {
    static let shared = PicachoSecurityKeys()

    /// As `picacho_ios_nfc_poll` tells Rust.
    enum State: Int32 {
        case looking = 0
        case connected = 1
        case cancelled = 2
        case ended = 3
    }

    private let queue = DispatchQueue(label: "tech.pinal.picacho.security-keys")
    private let lock = NSLock()
    private var session: NFCTagReaderSession?
    private var tag: NFCISO7816Tag?
    private var state = State.ended
    /// What the sheet says when another tag is held to it, or the key
    /// moved away too soon: Rust's words, in the app's language.
    private var notKey = ""
    private var holdStill = ""

    /// How long one exchange with the key may take.
    private static let exchange: TimeInterval = 30

    func begin(message: String, notKey: String, holdStill: String) -> Bool {
        guard NFCTagReaderSession.readingAvailable,
            let session = NFCTagReaderSession(pollingOption: .iso14443, delegate: self, queue: queue)
        else {
            return false
        }
        let earlier = lock.withLock { () -> NFCTagReaderSession? in
            let earlier = self.session
            self.session = session
            tag = nil
            state = .looking
            self.notKey = notKey
            self.holdStill = holdStill
            return earlier
        }
        earlier?.invalidate()
        session.alertMessage = message
        session.begin()
        return true
    }

    var current: State {
        lock.withLock { state }
    }

    func say(_ message: String) {
        lock.withLock { session }?.alertMessage = message
    }

    func end(error: String) {
        let session = lock.withLock { () -> NFCTagReaderSession? in
            let session = self.session
            self.session = nil
            tag = nil
            state = .ended
            return session
        }
        if error.isEmpty {
            session?.invalidate()
        } else {
            session?.invalidate(errorMessage: error)
        }
    }

    /// Sends `apdu` to the key and returns its answer: data, SW1, SW2.
    func transceive(_ apdu: Data) -> Data? {
        guard let tag = lock.withLock({ tag }), let command = NFCISO7816APDU(data: apdu) else {
            return nil
        }
        let done = DispatchSemaphore(value: 0)
        let answer = Answer()
        tag.sendCommand(apdu: command) { data, sw1, sw2, error in
            if error == nil {
                var whole = data
                whole.append(sw1)
                whole.append(sw2)
                answer.set(whole)
            }
            done.signal()
        }
        guard done.wait(timeout: .now() + Self.exchange) == .success else { return nil }
        return answer.get()
    }

    func tagReaderSessionDidBecomeActive(_ session: NFCTagReaderSession) {}

    func tagReaderSession(_ session: NFCTagReaderSession, didInvalidateWithError error: Error) {
        lock.withLock {
            guard session === self.session else { return }
            let cancelled = (error as? NFCReaderError)?.code == .readerSessionInvalidationErrorUserCanceled
            state = cancelled ? .cancelled : .ended
            tag = nil
            self.session = nil
        }
    }

    func tagReaderSession(_ session: NFCTagReaderSession, didDetect tags: [NFCTag]) {
        let key = tags.lazy.compactMap { tag -> NFCISO7816Tag? in
            if case let .iso7816(key) = tag { return key }
            return nil
        }.first
        guard let key else {
            session.alertMessage = lock.withLock { notKey }
            session.restartPolling()
            return
        }
        session.connect(to: .iso7816(key)) { [weak self] error in
            guard let self else { return }
            if error != nil {
                session.alertMessage = self.lock.withLock { self.holdStill }
                session.restartPolling()
                return
            }
            self.lock.withLock {
                guard session === self.session else { return }
                self.tag = key
                self.state = .connected
            }
        }
    }

    /// The answer an exchange's completion hands to the waiting thread.
    private final class Answer: @unchecked Sendable {
        private let lock = NSLock()
        private var data: Data?
        func set(_ data: Data) {
            lock.withLock { self.data = data }
        }
        func get() -> Data? {
            lock.withLock { data }
        }
    }
}

@_cdecl("picacho_ios_nfc_available")
public func picachoIosNfcAvailable() -> Bool {
    NFCTagReaderSession.readingAvailable
}

@_cdecl("picacho_ios_nfc_begin")
public func picachoIosNfcBegin(
    _ message: UnsafePointer<UInt8>?, _ len: Int,
    _ notKey: UnsafePointer<UInt8>?, _ notKeyLen: Int,
    _ holdStill: UnsafePointer<UInt8>?, _ holdStillLen: Int
) -> Bool {
    PicachoSecurityKeys.shared.begin(
        message: picachoString(message, len),
        notKey: picachoString(notKey, notKeyLen),
        holdStill: picachoString(holdStill, holdStillLen))
}

@_cdecl("picacho_ios_nfc_poll")
public func picachoIosNfcPoll() -> Int32 {
    PicachoSecurityKeys.shared.current.rawValue
}

@_cdecl("picacho_ios_nfc_transceive")
public func picachoIosNfcTransceive(
    _ apdu: UnsafePointer<UInt8>?, _ len: Int, _ out: UnsafeMutablePointer<UInt8>?, _ cap: Int
) -> Int {
    guard let apdu, let out, len > 0,
        let answer = PicachoSecurityKeys.shared.transceive(Data(bytes: apdu, count: len)),
        answer.count <= cap
    else {
        return -1
    }
    answer.copyBytes(to: out, count: answer.count)
    return answer.count
}

@_cdecl("picacho_ios_nfc_message")
public func picachoIosNfcMessage(_ message: UnsafePointer<UInt8>?, _ len: Int) {
    PicachoSecurityKeys.shared.say(picachoString(message, len))
}

@_cdecl("picacho_ios_nfc_end")
public func picachoIosNfcEnd(_ error: UnsafePointer<UInt8>?, _ len: Int) {
    PicachoSecurityKeys.shared.end(error: picachoString(error, len))
}
