//
//  SignalingClient.swift
//  Lan Mic
//
//  WebSocket signaling client using URLSessionWebSocketTask.
//  Exchanges offer/answer/ICE JSON with the Windows receiver.
//  Includes keepalive ping and exponential-backoff reconnect.
//

import Foundation
import os.log

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
    private let logger = Logger(subsystem: "com.lanmic.app", category: "Signaling")

    // Reconnect
    var autoReconnect = false
    private var reconnectAttempt = 0
    private let maxBackoff: TimeInterval = 5.0
    private var reconnectURL: URL?
    private var reconnectWorkItem: DispatchWorkItem?

    // Keepalive
    private var pingTimer: Timer?
    private static let pingInterval: TimeInterval = 10.0

    // MARK: - Connect / Disconnect

    func connect(to url: URL) {
        disconnect()
        reconnectURL = url
        reconnectAttempt = 0

        openConnection(to: url)
    }

    func disconnect() {
        reconnectWorkItem?.cancel()
        reconnectWorkItem = nil
        autoReconnect = false // Stop reconnect attempts on explicit disconnect
        stopPingTimer()
        webSocket?.cancel(with: .goingAway, reason: nil)
        webSocket = nil
        invalidateSession()
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
            logger.error("Failed to encode signaling message")
            return
        }
        webSocket?.send(.string(text)) { [weak self] error in
            if let error {
                self?.logger.error("Send error: \(error.localizedDescription)")
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
                self.logger.error("Receive error: \(error.localizedDescription)")
                self.handleDisconnect(error: error)
            }
        }
    }

    private func handleMessage(_ text: String) {
        guard let data = text.data(using: .utf8),
              let msg = try? JSONDecoder().decode(SignalingMessage.self, from: data)
        else {
            logger.warning("Failed to parse message: \(text.prefix(200))")
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
            logger.warning("Unknown message type: \(msg.type)")
        }
    }

    // MARK: - Keepalive Ping

    private func startPingTimer() {
        stopPingTimer()
        pingTimer = Timer.scheduledTimer(withTimeInterval: Self.pingInterval, repeats: true) { [weak self] _ in
            self?.sendPing()
        }
    }

    private func stopPingTimer() {
        pingTimer?.invalidate()
        pingTimer = nil
    }

    private func sendPing() {
        webSocket?.sendPing { [weak self] error in
            if let error {
                self?.logger.warning("Ping failed: \(error.localizedDescription)")
                self?.handleDisconnect(error: error)
            }
        }
    }

    // MARK: - Internal session management

    private func invalidateSession() {
        session?.invalidateAndCancel()
        session = nil
    }

    private func openConnection(to url: URL) {
        // Always clean up previous session to avoid leaks
        invalidateSession()

        let config = URLSessionConfiguration.default
        config.waitsForConnectivity = false
        config.timeoutIntervalForRequest = 10
        session = URLSession(configuration: config, delegate: self, delegateQueue: .main)
        webSocket = session?.webSocketTask(with: url)
        webSocket?.resume()
        listenForMessages()
    }

    // MARK: - Disconnect / Reconnect

    private func handleDisconnect(error: Error?) {
        guard isConnected || webSocket != nil else { return }
        isConnected = false
        webSocket = nil
        stopPingTimer()
        delegate?.signalingDidDisconnect(self, error: error)
        attemptReconnect()
    }

    private func attemptReconnect() {
        guard autoReconnect, let url = reconnectURL else { return }
        reconnectAttempt += 1
        let delay = min(pow(2.0, Double(reconnectAttempt - 1)) * 0.5, maxBackoff)
        logger.info("Reconnecting in \(delay)s (attempt \(self.reconnectAttempt))")

        let work = DispatchWorkItem { [weak self] in
            guard let self, self.autoReconnect else { return }
            self.openConnection(to: url)
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
        logger.info("WebSocket connected")
        isConnected = true
        reconnectAttempt = 0
        startPingTimer()
        delegate?.signalingDidConnect(self)
    }

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        logger.info("WebSocket closed: \(closeCode.rawValue)")
        handleDisconnect(error: nil)
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        if let error {
            logger.error("Task failed: \(error.localizedDescription)")
            handleDisconnect(error: error)
        }
    }
}
