//
//  ConnectionViewModel.swift
//  Lan Mic
//
//  Orchestrates AudioSessionManager, SignalingClient, and WebRTCManager.
//  Publishes state for SwiftUI consumption.
//

import Foundation
import Combine
import WebRTC

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

    @Published var ip: String = ""
    @Published var port: String = "9001"
    @Published var autoReconnect: Bool = false
    @Published var state: State = .idle
    @Published var iceState: String = "New"
    @Published var packetsSent: Int = 0
    @Published var lastError: String = ""

    // MARK: - Private

    private let signaling = SignalingClient()
    private let webRTC = WebRTCManager()
    private let audio = AudioSessionManager.shared

    private var statsTimer: Timer?
    private var signalingBridge: SignalingBridge?
    private var webRTCBridge: WebRTCBridge?

    // MARK: - Init

    nonisolated init() {}

    // MARK: - Actions

    func start() {
        guard state == .idle || state == .failed else { return }

        lastError = ""
        state = .connecting

        // Activate audio session
        do {
            try audio.activate()
        } catch {
            lastError = "Audio session error: \(error.localizedDescription)"
            state = .failed
            return
        }

        // Setup delegates (nonisolated bridging) — must retain bridges
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
            return
        }
        signaling.connect(to: url)
    }

    func stop() {
        statsTimer?.invalidate()
        statsTimer = nil
        signaling.disconnect()
        webRTC.close()
        audio.deactivate()
        signalingBridge = nil
        webRTCBridge = nil
        state = .idle
        iceState = "New"
        packetsSent = 0
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

    // MARK: - Signaling callbacks

    fileprivate func onSignalingConnected() {
        state = .exchangingSDP
    }

    fileprivate func onSignalingDisconnected(error: Error?) {
        if let error {
            lastError = error.localizedDescription
        }
        if state == .connected || state == .exchangingSDP || state == .iceConnecting {
            state = .failed
            webRTC.close()
            statsTimer?.invalidate()
            statsTimer = nil
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
            startStatsPolling()
        case .failed:
            state = .failed
            lastError = "Peer connection failed"
            statsTimer?.invalidate()
        case .disconnected:
            if state == .connected {
                state = .failed
                lastError = "Peer disconnected"
                statsTimer?.invalidate()
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
        print("[ViewModel] Unexpected answer received")
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
