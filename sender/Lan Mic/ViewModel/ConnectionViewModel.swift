//
//  ConnectionViewModel.swift
//  Lan Mic
//
//  Orchestrates AudioSessionManager, SignalingClient, WebRTCManager,
//  and ReceiverDiscovery. Publishes state for SwiftUI consumption.
//

import Foundation
import Combine
import WebRTC
import os.log

@MainActor
final class ConnectionViewModel: ObservableObject {

    // MARK: - Connection State

    enum State: String {
        case idle = "Idle"
        case connecting = "Connecting"
        case exchangingSDP = "Exchanging SDP"
        case iceConnecting = "ICE Connecting"
        case connected = "Connected"
        case failed = "Failed"
    }

    // MARK: - Published properties

    @Published var ip: String {
        didSet { UserDefaults.standard.set(ip, forKey: "lanmic_ip") }
    }
    @Published var port: String {
        didSet { UserDefaults.standard.set(port, forKey: "lanmic_port") }
    }
    @Published var autoReconnect: Bool = false
    @Published var state: State = .idle
    @Published var iceState: String = "New"
    @Published var packetsSent: Int = 0
    @Published var lastError: String = ""

    // MARK: - Discovery

    @Published var discovery = ReceiverDiscovery()

    // MARK: - Private

    private let signaling = SignalingClient()
    private let webRTC = WebRTCManager()
    private let audio = AudioSessionManager.shared

    private var statsTimer: Timer?
    private var connectionTimer: Timer?
    private var signalingBridge: SignalingBridge?
    private var webRTCBridge: WebRTCBridge?
    private var connectTime: Date?

    // MARK: - Logger
    private static let logger = Logger(subsystem: "com.lanmic.app", category: "ConnectionViewModel")

    // MARK: - Constants
    private static let connectionTimeoutSeconds: TimeInterval = 15

    // MARK: - Init

    nonisolated init() {
        // Load persisted values — must use a local then assign
        let savedIP = UserDefaults.standard.string(forKey: "lanmic_ip") ?? ""
        let savedPort = UserDefaults.standard.string(forKey: "lanmic_port") ?? "9001"
        _ip = Published(initialValue: savedIP)
        _port = Published(initialValue: savedPort)
    }

    // MARK: - Discovery Helpers

    func startDiscovery() {
        discovery.startBrowsing()
    }

    func stopDiscovery() {
        discovery.stopBrowsing()
    }

    func selectReceiver(_ receiver: DiscoveredReceiver) {
        guard !receiver.isResolving, !receiver.host.isEmpty else { return }
        ip = receiver.host
        port = "\(receiver.port)"
        Self.logger.info("Selected receiver: \(receiver.name) → \(receiver.host):\(receiver.port)")
    }

    // MARK: - Validation

    private func isValidIPv4(_ ip: String) -> Bool {
        let parts = ip.split(separator: ".")
        guard parts.count == 4 else { return false }
        return parts.allSatisfy { part in
            guard let num = Int(part) else { return false }
            return (0...255).contains(num)
        }
    }

    private func isValidPort(_ port: String) -> Bool {
        guard let portNum = Int(port) else { return false }
        return (1...65535).contains(portNum)
    }

    // MARK: - Actions

    func start() {
        guard state == .idle || state == .failed else { return }

        // Validate IP
        guard isValidIPv4(ip) else {
            lastError = "Invalid IP address. Expected: xxx.xxx.xxx.xxx"
            state = .failed
            Self.logger.error("Invalid IP address: \(self.ip)")
            return
        }

        // Validate port
        guard isValidPort(port) else {
            lastError = "Invalid port. Must be 1–65535"
            state = .failed
            Self.logger.error("Invalid port: \(self.port)")
            return
        }

        lastError = ""
        state = .connecting

        // Activate audio session
        do {
            try audio.activate()
        } catch {
            lastError = "Audio error: \(error.localizedDescription)"
            state = .failed
            Self.logger.error("Audio session activation failed: \(error.localizedDescription)")
            return
        }

        // Setup delegate bridges — retain them
        let sBridge = SignalingBridge(vm: self)
        self.signalingBridge = sBridge
        signaling.delegate = sBridge

        let wBridge = WebRTCBridge(vm: self)
        self.webRTCBridge = wBridge
        webRTC.delegate = wBridge

        // Configure reconnect
        signaling.autoReconnect = autoReconnect

        // Create peer connection
        webRTC.createPeerConnection()

        // Connect signaling
        guard let url = URL(string: "ws://\(ip):\(port)/ws") else {
            lastError = "Invalid URL"
            state = .failed
            Self.logger.error("Failed to create WebSocket URL")
            return
        }

        Self.logger.info("Connecting to \(url.absoluteString)")
        signaling.connect(to: url)

        // Start connection timeout
        startConnectionTimeout()
    }

    func stop() {
        cancelConnectionTimeout()
        statsTimer?.invalidate()
        statsTimer = nil
        connectTime = nil

        // Disable auto-reconnect BEFORE disconnecting to prevent race
        signaling.autoReconnect = false
        signaling.disconnect()
        webRTC.close()
        audio.deactivate()

        signalingBridge = nil
        webRTCBridge = nil
        state = .idle
        iceState = "New"
        packetsSent = 0
    }

    // MARK: - Connection timeout

    private func startConnectionTimeout() {
        cancelConnectionTimeout()
        connectionTimer = Timer.scheduledTimer(withTimeInterval: Self.connectionTimeoutSeconds, repeats: false) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self, self.state == .connecting || self.state == .exchangingSDP else { return }
                self.lastError = "Connection timed out after \(Int(Self.connectionTimeoutSeconds))s"
                Self.logger.warning("Connection timeout")
                self.stop()
                self.state = .failed
            }
        }
    }

    private func cancelConnectionTimeout() {
        connectionTimer?.invalidate()
        connectionTimer = nil
    }

    // MARK: - Stats polling

    private func startStatsPolling() {
        statsTimer?.invalidate()
        statsTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.webRTC.getStats { packets in
                    Task { @MainActor in
                        self.packetsSent = packets
                    }
                }
            }
        }
    }

    // MARK: - Connection Duration

    var connectionDuration: TimeInterval? {
        guard state == .connected, let connectTime else { return nil }
        return Date().timeIntervalSince(connectTime)
    }

    // MARK: - Signaling callbacks

    fileprivate func onSignalingConnected() {
        state = .exchangingSDP
        cancelConnectionTimeout()
    }

    fileprivate func onSignalingDisconnected(error: Error?) {
        if let error {
            lastError = error.localizedDescription
        }
        if state == .connected || state == .exchangingSDP || state == .iceConnecting {
            state = .failed
            webRTC.close()
            audio.deactivate()
            statsTimer?.invalidate()
            statsTimer = nil
            connectTime = nil
        }
    }

    fileprivate func onReceivedOffer(sdp: String) {
        state = .exchangingSDP
        webRTC.setRemoteOffer(sdp) { [weak self] error in
            Task { @MainActor [weak self] in
                guard let self else { return }
                if let error {
                    self.lastError = "Set offer error: \(error.localizedDescription)"
                    self.state = .failed
                    return
                }
                self.webRTC.createAnswer { [weak self] answerSDP, error in
                    Task { @MainActor [weak self] in
                        guard let self else { return }
                        if let error {
                            self.lastError = "Create answer error: \(error.localizedDescription)"
                            self.state = .failed
                            return
                        }
                        if let answerSDP {
                            self.signaling.sendAnswer(sdp: answerSDP)
                            self.state = .iceConnecting
                        }
                    }
                }
            }
        }
    }

    fileprivate func onReceivedICE(candidate: String, sdpMid: String?, sdpMLineIndex: Int32?) {
        webRTC.addICECandidate(candidate: candidate, sdpMid: sdpMid, sdpMLineIndex: sdpMLineIndex)
    }

    fileprivate func onICECandidate(_ candidate: RTCIceCandidate) {
        signaling.sendICECandidate(
            candidate: candidate.sdp,
            sdpMid: candidate.sdpMid,
            sdpMLineIndex: candidate.sdpMLineIndex
        )
    }

    fileprivate func onConnectionStateChanged(_ newState: RTCPeerConnectionState) {
        switch newState {
        case .connected:
            state = .connected
            connectTime = Date()
            startStatsPolling()
        case .failed:
            state = .failed
            lastError = "Peer connection failed"
            statsTimer?.invalidate()
            statsTimer = nil
            connectTime = nil
        case .disconnected:
            if state == .connected {
                state = .failed
                lastError = "Peer disconnected"
                statsTimer?.invalidate()
                statsTimer = nil
                connectTime = nil
            }
        case .closed:
            if state != .idle {
                state = .idle
            }
        default:
            break
        }
    }

    fileprivate func onICEStateChanged(_ newState: RTCIceConnectionState) {
        switch newState {
        case .new: iceState = "New"
        case .checking: iceState = "Checking"
        case .connected: iceState = "Connected"
        case .completed: iceState = "Completed"
        case .failed: iceState = "Failed"
        case .disconnected: iceState = "Disconnected"
        case .closed: iceState = "Closed"
        case .count: iceState = "Count"
        @unknown default: iceState = "Unknown"
        }
    }
}

// MARK: - Signaling Bridge (nonisolated delegate adapter)

private final class SignalingBridge: SignalingClientDelegate {
    private weak var vm: ConnectionViewModel?
    init(vm: ConnectionViewModel) { self.vm = vm }

    func signalingDidConnect(_ client: SignalingClient) {
        Task { @MainActor [weak self] in self?.vm?.onSignalingConnected() }
    }

    func signalingDidDisconnect(_ client: SignalingClient, error: Error?) {
        Task { @MainActor [weak self] in self?.vm?.onSignalingDisconnected(error: error) }
    }

    func signaling(_ client: SignalingClient, didReceiveOffer sdp: String) {
        Task { @MainActor [weak self] in self?.vm?.onReceivedOffer(sdp: sdp) }
    }

    func signaling(_ client: SignalingClient, didReceiveAnswer sdp: String) {
        // iOS is the answerer — should not normally receive an answer
        ConnectionViewModel.logger.warning("Unexpected answer received from server")
    }

    func signaling(_ client: SignalingClient, didReceiveICE candidate: String, sdpMid: String?, sdpMLineIndex: Int32?) {
        Task { @MainActor [weak self] in self?.vm?.onReceivedICE(candidate: candidate, sdpMid: sdpMid, sdpMLineIndex: sdpMLineIndex) }
    }
}

// MARK: - WebRTC Bridge (nonisolated delegate adapter)

private final class WebRTCBridge: WebRTCManagerDelegate {
    private weak var vm: ConnectionViewModel?
    init(vm: ConnectionViewModel) { self.vm = vm }

    func webRTCManager(_ manager: WebRTCManager, didGenerateICECandidate candidate: RTCIceCandidate) {
        Task { @MainActor [weak self] in self?.vm?.onICECandidate(candidate) }
    }

    func webRTCManager(_ manager: WebRTCManager, didChangeConnectionState state: RTCPeerConnectionState) {
        Task { @MainActor [weak self] in self?.vm?.onConnectionStateChanged(state) }
    }

    func webRTCManager(_ manager: WebRTCManager, didChangeICEState state: RTCIceConnectionState) {
        Task { @MainActor [weak self] in self?.vm?.onICEStateChanged(state) }
    }

    func webRTCManager(_ manager: WebRTCManager, didChangeSignalingState state: RTCSignalingState) {
        // Logged in WebRTCManager
    }
}
