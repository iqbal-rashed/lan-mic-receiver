//
//  SignalingClient.swift
//  Lan Mic
//
//  WebSocket signaling client using URLSessionWebSocketTask.
//  Exchanges offer/answer/ICE JSON with the Windows receiver.
//

import Foundation

// MARK: - Delegate

protocol SignalingClientDelegate: AnyObject {
    func signaling(_ client: SignalingClient, didReceiveOffer sdp: String)
    func signaling(_ client: SignalingClient, didReceiveAnswer sdp: String)
    func signaling(_ client: SignalingClient, didReceiveICE candidate: String, sdpMid: String?, sdpMLineIndex: Int32?)
    func signalingDidConnect(_ client: SignalingClient)
    func signalingDidDisconnect(_ client: SignalingClient, error: Error?)
}

// MARK: - Message models

private struct SignalingMessage: Codable {
    let type: String
    var sdp: String?
    var candidate: String?
    var sdpMid: String?
    var sdpMLineIndex: Int32?
}

// MARK: - SignalingClient

final class SignalingClient: NSObject {
    weak var delegate: SignalingClientDelegate?

    private var webSocket: URLSessionWebSocketTask?
    private var session: URLSession?
    private var isConnected = false

    // Reconnect
    var autoReconnect = false
    private var reconnectAttempt = 0
    private let maxBackoff: TimeInterval = 5.0
    private var reconnectURL: URL?
    private var reconnectWorkItem: DispatchWorkItem?

    // MARK: - Connect / Disconnect

    func connect(to url: URL) {
        disconnect()
        reconnectURL = url
        reconnectAttempt = 0

        let config = URLSessionConfiguration.default
        config.waitsForConnectivity = false
        session = URLSession(configuration: config, delegate: self, delegateQueue: .main)
        webSocket = session?.webSocketTask(with: url)
        webSocket?.resume()
        listenForMessages()
    }

    func disconnect() {
        reconnectWorkItem?.cancel()
        reconnectWorkItem = nil
        autoReconnect = false // Stop reconnect attempts on explicit disconnect
        webSocket?.cancel(with: .goingAway, reason: nil)
        webSocket = nil
        session?.invalidateAndCancel()
        session = nil
        isConnected = false
    }

    // MARK: - Send

    func sendAnswer(sdp: String) {
        let msg = SignalingMessage(type: "answer", sdp: sdp)
        send(msg)
    }

    func sendICECandidate(candidate: String, sdpMid: String?, sdpMLineIndex: Int32?) {
        let msg = SignalingMessage(
            type: "ice",
            candidate: candidate,
            sdpMid: sdpMid,
            sdpMLineIndex: sdpMLineIndex
        )
        send(msg)
    }

    private func send(_ message: SignalingMessage) {
        guard let data = try? JSONEncoder().encode(message),
              let text = String(data: data, encoding: .utf8)
        else {
            print("[Signaling] Failed to encode message")
            return
        }
        webSocket?.send(.string(text)) { error in
            if let error {
                print("[Signaling] Send error: \(error.localizedDescription)")
            }
        }
    }

    // MARK: - Receive loop

    private func listenForMessages() {
        webSocket?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let message):
                switch message {
                case .string(let text):
                    self.handleMessage(text)
                case .data(let data):
                    if let text = String(data: data, encoding: .utf8) {
                        self.handleMessage(text)
                    }
                @unknown default:
                    break
                }
                self.listenForMessages() // Continue listening
            case .failure(let error):
                print("[Signaling] Receive error: \(error.localizedDescription)")
                self.handleDisconnect(error: error)
            }
        }
    }

    private func handleMessage(_ text: String) {
        guard let data = text.data(using: .utf8),
              let msg = try? JSONDecoder().decode(SignalingMessage.self, from: data)
        else {
            print("[Signaling] Failed to parse message: \(text)")
            return
        }

        switch msg.type {
        case "offer":
            if let sdp = msg.sdp {
                delegate?.signaling(self, didReceiveOffer: sdp)
            }
        case "answer":
            if let sdp = msg.sdp {
                delegate?.signaling(self, didReceiveAnswer: sdp)
            }
        case "ice":
            if let candidate = msg.candidate {
                delegate?.signaling(self, didReceiveICE: candidate, sdpMid: msg.sdpMid, sdpMLineIndex: msg.sdpMLineIndex)
            }
        default:
            print("[Signaling] Unknown message type: \(msg.type)")
        }
    }

    // MARK: - Disconnect / Reconnect

    private func handleDisconnect(error: Error?) {
        guard isConnected || webSocket != nil else { return }
        isConnected = false
        webSocket = nil
        delegate?.signalingDidDisconnect(self, error: error)
        attemptReconnect()
    }

    private func attemptReconnect() {
        guard autoReconnect, let url = reconnectURL else { return }
        reconnectAttempt += 1
        let delay = min(pow(2.0, Double(reconnectAttempt - 1)) * 0.5, maxBackoff)
        print("[Signaling] Reconnecting in \(delay)s (attempt \(reconnectAttempt))")

        let work = DispatchWorkItem { [weak self] in
            guard let self, self.autoReconnect else { return }
            let config = URLSessionConfiguration.default
            config.waitsForConnectivity = false
            self.session = URLSession(configuration: config, delegate: self, delegateQueue: .main)
            self.webSocket = self.session?.webSocketTask(with: url)
            self.webSocket?.resume()
            self.listenForMessages()
        }
        reconnectWorkItem = work
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work)
    }
}

// MARK: - URLSessionWebSocketDelegate

extension SignalingClient: URLSessionWebSocketDelegate {
    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didOpenWithProtocol protocol: String?
    ) {
        print("[Signaling] WebSocket connected")
        isConnected = true
        reconnectAttempt = 0
        delegate?.signalingDidConnect(self)
    }

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        print("[Signaling] WebSocket closed: \(closeCode.rawValue)")
        handleDisconnect(error: nil)
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        if let error {
            print("[Signaling] Task failed: \(error.localizedDescription)")
            handleDisconnect(error: error)
        }
    }
}
